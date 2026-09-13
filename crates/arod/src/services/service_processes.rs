//! Out-of-process Android services, including external isolated WebView renderers.
use super::activity::{ActivityService, ServiceBinding};
use super::registry::{AppSpec, Registry};
use crate::{aparcel as ap, parcelables::{CompatibilityInfo, ServiceInfo}, pending_intent::Target};
use anyhow::{Context, Result};
use rsbinder::{SIBinder, FLAG_ONEWAY};
use std::{collections::HashMap, ffi::OsString, path::PathBuf, process::Child, sync::Mutex};

#[derive(Default)]
pub struct ServiceProcesses {
    pub launcher: Mutex<Option<Launcher>>,
    pub processes: Mutex<HashMap<i64, ServiceProcess>>,
    pub app_pid: std::sync::atomic::AtomicI32,
}

pub struct Launcher {
    pub exe: PathBuf,
    pub env: Vec<(OsString, OsString)>,
}

impl ServiceProcess {
    pub fn is_bound_token(&self, token: &SIBinder) -> bool {
        &self.token == token && !self.pending.is_empty()
    }
}

pub struct ServiceProcess {
    key: (String, String, String),
    pub spec: AppSpec,
    info: ServiceInfo,
    token: SIBinder,
    pub thread: Option<SIBinder>,
    ready: bool,
    pending: Vec<(Target, SIBinder)>,
    child: Child,
}

impl ActivityService {
    pub fn bind_remote_service(&self, target: &Target, connection: SIBinder,
        instance: Option<&str>, caller: &str, external: bool) -> Result<bool> {
        let provider = target.package.as_deref().and_then(|p| self.registry.find(p))
            .context("unknown service package")?;
        let class = target.class.as_deref().context("service class missing")?;
        let mut info = Registry::service_info(&provider, class).context("undeclared service")?;
        let isolated = info.flags & crate::parcelables::SERVICE_FLAG_ISOLATED_PROCESS != 0;
        let owner = self.registry.find(caller).context("unknown caller package")?;
        if external && (info.flags & crate::parcelables::SERVICE_FLAG_EXTERNAL_SERVICE == 0 || !info.exported) {
            anyhow::bail!("external binding requires an exported external service");
        }
        let key = (caller.to_string(), class.to_string(), instance.unwrap_or("").to_string());
        let mut processes = self.service_processes.processes.lock().unwrap();
        let existing = processes.iter().find(|(_, p)| p.key == key).map(|(seq, _)| *seq);
        if let Some(seq) = existing {
            let process = processes.get_mut(&seq).unwrap();
            if process.child.try_wait()?.is_some() {
                processes.remove(&seq);
            } else {
                let mut target = target.clone();
                if external { target.package = Some(caller.to_string()); }
                if process.ready {
                    self.send_service_binding(process.thread.as_ref().unwrap(), &process.token, &target, connection)?;
                } else {
                    process.pending.push((target, connection));
                }
                return Ok(true);
            }
        }
        static NEXT_SEQ: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(1);
        let seq = NEXT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        anyhow::ensure!(seq < 1000, "isolated UID range exhausted");
        let uid = if isolated { 99000 + seq - 1 } else { i64::from(provider.uid) };
        let mut spec = provider.clone();
        if external {
            // External service code belongs to the provider; its context and data
            // belong to the embedding app (ActiveServices external-service semantics).
            spec.package = owner.package.clone();
            spec.uid = owner.uid;
        }
        spec.process = Some(if isolated {
            format!("{}:isolated:{}:{}", spec.package, class, instance.unwrap_or("0"))
        } else { info.process_name.clone() });
        spec.providers.clear();
        spec.main_activity = None;
        info.application_info = Registry::application_info(&spec);
        info.package_name = spec.package.clone();
        info.process_name = spec.process.clone().unwrap();
        let layout = aro_exec::layout::Layout::default();
        let host_apk = if provider.apk_in_ns.starts_with("/system/") {
            layout.system.join(provider.apk_in_ns.trim_start_matches('/'))
        } else {
            layout.data.join(provider.apk_in_ns.trim_start_matches("/data/"))
        };
        for root in ["data", "user_de/0"] {
            for sub in ["cache", "code_cache", "files", "databases", "shared_prefs"] {
                std::fs::create_dir_all(layout.data.join(root).join(&spec.package).join(sub))?;
            }
        }
        let launch = self.service_processes.launcher.lock().unwrap();
        let launch = launch.as_ref().context("service launcher not configured")?;
        let pid = self.service_processes.app_pid.load(std::sync::atomic::Ordering::Acquire);
        anyhow::ensure!(pid > 0, "app PID namespace not ready");
        let child = std::process::Command::new(&launch.exe)
            .envs(launch.env.iter().map(|(k, v)| (k, v)))
            .env("ARO_START_SEQ", seq.to_string())
            .env("ARO_APP_UID", uid.to_string())
            .env("ARO_PID_NAMESPACE", format!("/proc/{pid}/ns/pid"))
            .args(["run", "--app"]).arg(host_apk).spawn().context("launch isolated service")?;
        log::info!("activity: launched service process {} seq={seq} uid={uid} supervisor={}", info.process_name, child.id());
        let mut target = target.clone();
        if external { target.package = Some(caller.to_string()); }
        processes.insert(seq, ServiceProcess {
            key, spec, info, token: super::token::new_token("isolated-service"),
            thread: None, ready: false, pending: vec![(target, connection)], child,
        });
        Ok(true)
    }

    pub fn finish_service_attach(&self, seq: i64) -> Result<()> {
        let mut processes = self.service_processes.processes.lock().unwrap();
        let process = processes.get_mut(&seq).context("unknown isolated start sequence")?;
        let thread = process.thread.as_ref().context("isolated process not attached")?;
        let proxy = thread.as_proxy().context("isolated thread is not a proxy")?;
        let mut d = proxy.prepare_transact(true)?;
        d.write(&Some(process.token.clone()))?;
        ap::typed(&mut d, |p| process.info.write(p))?;
        ap::typed(&mut d, CompatibilityInfo::write_default)?;
        d.write_i32(4)?;
        proxy.submit_transact(4, &d, FLAG_ONEWAY)?;
        process.ready = true;
        log::info!("activity: service process attached seq={seq}; creating {}", process.info.name);
        for (target, connection) in process.pending.drain(..) {
            self.send_service_binding(thread, &process.token, &target, connection)?;
        }
        Ok(())
    }

    pub fn unbind_service_connection(&self, connection: &SIBinder) -> Result<()> {
        // Keep the process -> bindings lock order used by attach/bind.
        let mut processes = self.service_processes.processes.lock().unwrap();
        for process in processes.values_mut() {
            process.pending.retain(|(_, c)| c != connection);
        }
        let mut bindings = self.bindings.lock().unwrap();
        let mut removed = Vec::new();
        bindings.retain(|b| {
            if &b.connection == connection { removed.push(b.clone()); false } else { true }
        });
        for binding in &removed {
            if let Some(proxy) = binding.thread.as_proxy() {
                let mut d = proxy.prepare_transact(true)?;
                d.write(&Some(binding.service_token.clone()))?;
                d.write(&Some(binding.bind_token.clone()))?;
                ap::typed(&mut d, |p| binding.intent.write(p))?;
                proxy.submit_transact(13, &d, FLAG_ONEWAY)?;
            }
        }
        let finished: Vec<_> = processes.iter().filter(|(_, process)| {
            process.pending.is_empty() && !bindings.iter().any(|b| b.service_token == process.token)
        }).map(|(seq, _)| *seq).collect();
        drop(bindings);
        for seq in finished {
            let mut process = processes.remove(&seq).unwrap();
            if let Some(proxy) = process.thread.as_ref().and_then(|t| t.as_proxy()) {
                let mut d = proxy.prepare_transact(true)?;
                d.write(&Some(process.token.clone()))?;
                let _ = proxy.submit_transact(5, &d, FLAG_ONEWAY);
            }
            // Reap outside Binder threads; force cleanup if an Android service
            // does not finish its onDestroy/exit within the grace period.
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                loop {
                    match process.child.try_wait() {
                        Ok(Some(status)) => { log::info!("activity: service process seq={seq} exited {status}"); break; }
                        _ if std::time::Instant::now() >= deadline => {
                            // ActivityThread's main looper is not quittable;
                            // terminate the process after delivering onDestroy.
                            let _ = process.child.kill();
                            let _ = process.child.wait();
                            log::info!("activity: stopped unbound service process seq={seq}");
                            break;
                        }
                        _ => std::thread::sleep(std::time::Duration::from_millis(20)),
                    }
                }
            });
        }
        Ok(())
    }

    pub fn send_service_binding(&self, thread: &SIBinder, token: &SIBinder, target: &Target, connection: SIBinder) -> Result<()> {
        let proxy = thread.as_proxy().context("service thread is not a proxy")?;
        let bind_token = super::token::new_token("bind");
        self.bindings.lock().unwrap().push(ServiceBinding {
            bind_token: bind_token.clone(), service_token: token.clone(), intent: target.to_intent(), thread: thread.clone(), connection,
            package: target.package.clone().unwrap_or_default(),
            class: target.class.clone().unwrap_or_default(),
        });
        let mut d = proxy.prepare_transact(true)?;
        d.write(&Some(token.clone()))?;
        d.write(&Some(bind_token))?;
        ap::typed(&mut d, |p| target.to_intent().write(p))?;
        ap::boolean(&mut d, false)?;
        d.write_i32(4)?;
        d.write_i64(0)?;
        proxy.submit_transact(12, &d, FLAG_ONEWAY)?;
        Ok(())
    }
}
