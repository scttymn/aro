//! `activity`: android.app.IActivityManager.
//!
//! The app's `ActivityThread.main` calls `attachApplication(thread, seq)`. We
//! answer by calling back `IApplicationThread.bindApplication(...)` with the
//! app's ApplicationInfo and a desktop Configuration; the app then builds its
//! Context and runs `Application.onCreate()`.
use super::registry::{AppSpec, Registry};
use super::{codes, Service};
use crate::aparcel as ap;
use crate::parcelables::{CompatibilityInfo, Configuration};
use rsbinder::{Parcel, Result, SIBinder, TransactionCode, FLAG_ONEWAY};
use std::sync::{Arc, Mutex};

pub struct ActivityService {
    pub registry: Arc<Registry>,
    pub pending: Mutex<Option<AppSpec>>,
    /// Attached app process: its IApplicationThread and spec.
    pub attached: Mutex<Option<(SIBinder, AppSpec)>>,
    /// IActivityClientController binder handed to launched activities.
    pub client_controller: Mutex<Option<SIBinder>>,
}

const BIND_APPLICATION: u32 = 6; // IApplicationThread.bindApplication (spec/transactions.rs)
const SCHEDULE_TRANSACTION: u32 = 56; // IApplicationThread.scheduleTransaction

impl ActivityService {
    /// IApplicationThread.bindApplication, argument order from
    /// `IApplicationThread$Stub$Proxy.bindApplication` in the image.
    fn bind_application(thread: &SIBinder, spec: &AppSpec, display: (i32, i32, i32)) -> anyhow::Result<()> {
        let proxy = thread.as_proxy().ok_or_else(|| anyhow::anyhow!("IApplicationThread is not a proxy"))?;
        let mut d = proxy.prepare_transact(true)?;
        let ai = Registry::application_info(spec);
        let (w, h, dpi) = display;
        ap::string16(&mut d, Some(&spec.package))?; // processName
        ap::typed(&mut d, |p| ai.write(p))?; // ApplicationInfo
        ap::string16(&mut d, None)?; // sdkSandboxClientAppVolumeUuid
        ap::string16(&mut d, None)?; // sdkSandboxClientAppPackage
        ap::boolean(&mut d, false)?; // isSdkInSandbox
        ap::typed(&mut d, |p| {
            // ProviderInfoList: allowSquashing; writeTypedList(list)
            ap::empty_list(p)
        })?;
        ap::typed_none(&mut d)?; // instrumentationName ComponentName
        ap::typed_none(&mut d)?; // ProfilerInfo
        ap::typed_none(&mut d)?; // instrumentationArgs Bundle
        d.write(&None::<SIBinder>)?; // IInstrumentationWatcher
        d.write(&None::<SIBinder>)?; // IUiAutomationConnection
        d.write_i32(0)?; // debugMode: DEBUG_OFF
        ap::boolean(&mut d, false)?; // enableBinderTracking
        ap::boolean(&mut d, false)?; // trackAllocation
        ap::boolean(&mut d, false)?; // restrictedBackupMode
        ap::boolean(&mut d, false)?; // persistent
        ap::typed(&mut d, |p| Configuration::desktop(w, h, dpi).write(p))?; // Configuration
        ap::typed(&mut d, CompatibilityInfo::write_default)?; // CompatibilityInfo
        ap::empty_list(&mut d)?; // services Map: empty
        ap::typed_none(&mut d)?; // coreSettings Bundle (typed object): null
        ap::string16(&mut d, Some("unknown"))?; // buildSerial
        ap::typed_none(&mut d)?; // AutofillOptions
        ap::typed_none(&mut d)?; // ContentCaptureOptions
        ap::long_array(&mut d, Some(&[]))?; // disabledCompatChanges
        ap::long_array(&mut d, Some(&[]))?; // loggableCompatChanges
        ap::long_array(&mut d, Some(&[]))?; // ? third long[]
        ap::boolean(&mut d, false)?; // ?
        ap::typed_none(&mut d)?; // serializedSystemFontMap SharedMemory
        // applicationSharedMemoryFd: a zero-filled region is a valid "nothing cached" state.
        let shm = crate::shm::application_shared_memory()?;
        d.write_raw_file_descriptor(std::os::fd::AsFd::as_fd(&shm))?;
        d.write_i64(0)?; // startRequestedElapsedTime
        d.write_i64(0)?; // startRequestedUptime
        if std::env::var_os("ARO_DUMP_PARCEL").is_some() {
            let bytes = unsafe { std::slice::from_raw_parts(d.as_ptr(), d.data_size()) };
            log::info!("bindApplication parcel ({} bytes): {}", bytes.len(), bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
        }
        proxy.submit_transact(BIND_APPLICATION, &d, FLAG_ONEWAY)?;
        Ok(())
    }
}

/// Shared handle so `main` can hand the pending launch to the service.
pub struct ActivityRef(pub std::sync::Arc<ActivityService>);

impl ActivityService {
    /// IApplicationThread.scheduleTransaction with LaunchActivityItem + ResumeActivityItem.
    fn launch_activity(thread: &SIBinder, spec: &AppSpec, controller: &SIBinder, display: (i32, i32, i32), activity: &str, intent: crate::parcelables::Intent) -> anyhow::Result<()> {
        use crate::parcelables::{LaunchTransaction, Rect};
        let proxy = thread.as_proxy().ok_or_else(|| anyhow::anyhow!("IApplicationThread is not a proxy"))?;
        let (w, h, dpi) = display;
        let config = Configuration::desktop(w, h, dpi);
        let info = Registry::activity_info(spec, activity).ok_or_else(|| anyhow::anyhow!("activity {activity} not declared in manifest"))?;
        let activity_token = super::token::new_token("activity");
        let assist_token = super::token::new_token("assist");
        let shareable_token = super::token::new_token("shareable");
        let tx = LaunchTransaction {
            activity_token: &activity_token,
            assist_token: &assist_token,
            shareable_token: &shareable_token,
            client_controller: controller,
            ident: 1,
            config: &config,
            intent: &intent,
            info: &info,
            display_id: 0,
            task_bounds: Rect { left: 0, top: 0, right: w, bottom: h },
        };
        let mut d = proxy.prepare_transact(true)?;
        tx.write(&mut d)?;
        // Keep the tokens alive for the app's lifetime (leak on purpose for now).
        std::mem::forget((activity_token, assist_token, shareable_token));
        proxy.submit_transact(SCHEDULE_TRANSACTION, &d, FLAG_ONEWAY)?;
        Ok(())
    }
}

impl Service for ActivityRef {
    const DESCRIPTOR: &'static str = "android.app.IActivityManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYMANAGER;
    fn handle(&self, name: &str, code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        self.0.handle(name, code, data, reply)
    }
}

impl ActivityService {
    /// Dispatch a startActivity: resolve the target and launch it into the
    /// running app. Explicit (component set) same-app intents only for now.
    pub fn start_activity(&self, pkg: Option<String>, class: Option<String>, action: Option<String>) {
        let attached = self.attached.lock().unwrap().clone();
        let controller = self.client_controller.lock().unwrap().clone();
        let display = self.registry.display;
        let (Some((thread, spec)), Some(controller)) = (attached, controller) else {
            log::warn!("activity: startActivity with no running app");
            return;
        };
        let target = match (pkg.as_deref(), class) {
            (Some(p), Some(c)) if p == spec.package => c,
            (Some(p), Some(_)) => {
                log::warn!("activity: startActivity to another package {p:?} not supported yet");
                return;
            }
            _ => {
                // Implicit intent: resolve against this app's launcher for now.
                log::warn!("activity: implicit startActivity (action={action:?}) — resolving to main activity");
                match spec.main_activity.clone() {
                    Some(m) => m,
                    None => return,
                }
            }
        };
        if !spec.activities.iter().any(|a| a.name == target) {
            log::warn!("activity: startActivity target {target} not declared");
            return;
        }
        let intent = crate::parcelables::Intent {
            action,
            package: None,
            component: Some((spec.package.clone(), target.clone())),
            categories: vec![],
            flags: crate::parcelables::FLAG_ACTIVITY_NEW_TASK,
        };
        log::info!("activity: startActivity -> {}/{target}", spec.package);
        std::thread::spawn(move || {
            if let Err(e) = Self::launch_activity(&thread, &spec, &controller, display, &target, intent) {
                log::error!("activity: startActivity launch failed: {e:#}");
            }
        });
    }

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "attachApplication" => {
                let thread: Option<SIBinder> = data.read()?;
                let start_seq = data.read_i64()?;
                let pid = rsbinder::thread_state::get_calling_pid();
                log::info!("activity: attachApplication from pid {pid}, seq {start_seq}");
                ap::no_exception(reply)?;
                if let (Some(thread), Some(spec)) = (thread, self.pending.lock().unwrap().take()) {
                    *self.attached.lock().unwrap() = Some((thread.clone(), spec.clone()));
                    // Outbound call after we have replied: do it on another thread.
                    let display = self.registry.display;
                    std::thread::spawn(move || {
                        if let Err(e) = Self::bind_application(&thread, &spec, display) {
                            log::error!("activity: bindApplication failed: {e:#}");
                        } else {
                            log::info!("activity: bindApplication sent to {}", spec.package);
                        }
                    });
                }
                Ok(true)
            }
            "finishAttachApplication" => {
                let _seq = data.read_i64()?;
                let _oncreate_ns = data.read_i64()?;
                log::info!("activity: finishAttachApplication (Application.onCreate done)");
                ap::no_exception(reply)?;
                // The process is up: launch its main activity, as ActivityTaskManager would.
                let attached = self.attached.lock().unwrap().clone();
                let controller = self.client_controller.lock().unwrap().clone();
                let display = self.registry.display;
                if let (Some((thread, spec)), Some(controller)) = (attached, controller) {
                    let Some(main) = spec.main_activity.clone() else {
                        log::error!("activity: no main activity for {}", spec.package);
                        return Ok(true);
                    };
                    let intent = crate::parcelables::Intent {
                        action: Some("android.intent.action.MAIN".into()),
                        package: None,
                        component: Some((spec.package.clone(), main.clone())),
                        categories: vec!["android.intent.category.LAUNCHER".into()],
                        flags: crate::parcelables::FLAG_ACTIVITY_NEW_TASK,
                    };
                    std::thread::spawn(move || match Self::launch_activity(&thread, &spec, &controller, display, &main, intent) {
                        Ok(()) => log::info!("activity: launch transaction sent for {}", spec.package),
                        Err(e) => log::error!("activity: launch failed: {e:#}"),
                    });
                }
                Ok(true)
            }
            "frozenBinderTransactionDetected" => Ok(true), // oneway
            "handleApplicationCrash" => {
                let _app: Option<SIBinder> = data.read()?;
                let present = data.read_i32()?;
                let mut summary = String::new();
                if present != 0 {
                    let _handler: Option<String> = data.read()?;
                    let class: Option<String> = data.read()?;
                    let message: Option<String> = data.read()?;
                    let _file: Option<String> = data.read()?;
                    let _cls: Option<String> = data.read()?;
                    let _method: Option<String> = data.read()?;
                    let _line = data.read_i32()?;
                    let stack: Option<String> = data.read().unwrap_or(None);
                    summary = format!("{}: {}\n{}", class.unwrap_or_default(), message.unwrap_or_default(), stack.unwrap_or_default().lines().take(12).collect::<Vec<_>>().join("\n"));
                }
                log::error!("activity: app crashed: {summary}");
                ap::no_exception(reply)?;
                Ok(true)
            }
            "handleApplicationWtf" => {
                let _app: Option<SIBinder> = data.read()?;
                let tag: Option<String> = data.read()?;
                let _system = data.read_i32()?;
                let present = data.read_i32()?;
                let mut summary = String::new();
                if present != 0 {
                    // ApplicationErrorReport.CrashInfo (spec: crashinfo2.txt)
                    let _handler: Option<String> = data.read()?;
                    let class: Option<String> = data.read()?;
                    let message: Option<String> = data.read()?;
                    let _file: Option<String> = data.read()?;
                    let _cls: Option<String> = data.read()?;
                    let _method: Option<String> = data.read()?;
                    let _line = data.read_i32()?;
                    let stack: Option<String> = data.read().unwrap_or(None);
                    summary = format!("{}: {} | {}", class.unwrap_or_default(), message.unwrap_or_default(), stack.unwrap_or_default().lines().skip(1).take(3).map(str::trim).collect::<Vec<_>>().join(" <- "));
                }
                log::warn!("activity: WTF [{}] {summary}", tag.unwrap_or_default());
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?; // do not kill
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

