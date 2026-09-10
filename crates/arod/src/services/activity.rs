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
    pub compat: Arc<super::compat::PlatformCompatService>,
    pub pending: Mutex<Option<AppSpec>>,
    /// Attached app process: its IApplicationThread and spec.
    pub attached: Mutex<Option<(SIBinder, AppSpec)>>,
    /// IActivityClientController binder handed to launched activities.
    pub client_controller: Mutex<Option<SIBinder>>,
    pub settings_provider: SIBinder,
    pub media_provider: SIBinder,
    pub media_service: Arc<super::media::MediaService>,
    pub calendar_provider: SIBinder,
    #[allow(dead_code)]
    pub calendar_service: Arc<super::calendar::CalendarService>,
    /// Deep-link URI for the initial launch (arod app --url); ACTION_VIEW instead of MAIN.
    pub launch_url: std::sync::Mutex<Option<String>>,
    /// PendingIntents minted via getIntentSender (for notification tap-back).
    pub pending_intents: std::sync::Arc<crate::pending_intent::Registry>,
    /// Running activities in this task backstack.
    pub activity_stack: Mutex<Vec<String>>,
    /// Running services (class name -> token binder).
    pub running_services: Mutex<std::collections::HashMap<String, SIBinder>>,
    /// Monotonic service start counter.
    pub next_service_start_id: std::sync::atomic::AtomicI32,
}

const SCHEDULE_CREATE_SERVICE: u32 = 4; // IApplicationThread.scheduleCreateService
const SCHEDULE_STOP_SERVICE: u32 = 5; // IApplicationThread.scheduleStopService
const BIND_APPLICATION: u32 = 6; // IApplicationThread.bindApplication (spec/transactions.rs)
const SCHEDULE_SERVICE_ARGS: u32 = 9; // IApplicationThread.scheduleServiceArgs
const SCHEDULE_TRANSACTION: u32 = 56; // IApplicationThread.scheduleTransaction

impl ActivityService {
    /// IApplicationThread.bindApplication, argument order from
    /// `IApplicationThread$Stub$Proxy.bindApplication` in the image.
    fn bind_application(
        thread: &SIBinder,
        spec: &AppSpec,
        display: (i32, i32, i32),
        disabled_compat: &[i64],
        enabled_compat: &[i64],
    ) -> anyhow::Result<()> {
        let proxy = thread.as_proxy().ok_or_else(|| anyhow::anyhow!("IApplicationThread is not a proxy"))?;
        let mut d = proxy.prepare_transact(true)?;
        let ai = Registry::application_info(spec);
        let (w, h, dpi) = display;
        ap::string16(&mut d, Some(&spec.package))?; // processName
        ap::typed(&mut d, |p| ai.write(p))?; // ApplicationInfo
        ap::string16(&mut d, None)?; // sdkSandboxClientAppVolumeUuid
        ap::string16(&mut d, None)?; // sdkSandboxClientAppPackage
        ap::boolean(&mut d, false)?; // isSdkInSandbox
        let providers: Vec<_> = spec.providers.iter()
            .map(|prov| Registry::provider_info(spec, prov))
            .collect();
        ap::typed(&mut d, |p| {
            // ProviderInfoList: allowSquashing; writeTypedList(list)
            p.write_i32(providers.len() as i32)?;
            for prov in &providers {
                p.write_i32(1)?; // non-null
                prov.write(p)?;
            }
            Ok(())
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
        ap::long_array(&mut d, Some(disabled_compat))?; // disabledCompatChanges
        ap::long_array(&mut d, Some(enabled_compat))?; // enabledCompatChanges
        ap::long_array(&mut d, Some(&[]))?; // mLoggableCompatChanges
        ap::boolean(&mut d, false)?; // mLogChangeChecksToStatsD
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

    /// Schedule DestroyActivityItem via IApplicationThread.scheduleTransaction.
    pub fn destroy_activity(&self, activity_token: SIBinder) {
        let attached = self.attached.lock().unwrap().clone();
        let Some((thread, _spec)) = attached else {
            log::warn!("activity: destroy_activity with no attached thread");
            return;
        };
        std::thread::spawn(move || {
            let Some(proxy) = thread.as_proxy() else {
                log::error!("activity: IApplicationThread is not a proxy");
                return;
            };
            if let Ok(mut d) = proxy.prepare_transact(true) {
                // ClientTransaction (writeTypedObject)
                let _ = d.write_i32(1); // typed object marker (ClientTransaction != null)
                let _ = d.write_i32(1); // 1 item in list
                // DestroyActivityItem
                let _ = ap::string16(&mut d, Some("android.app.servertransaction.DestroyActivityItem"));
                let _ = d.write(&Some(activity_token));
                let _ = ap::boolean(&mut d, true); // mFinished = true
                if let Err(e) = proxy.submit_transact(SCHEDULE_TRANSACTION, &d, FLAG_ONEWAY) {
                    log::error!("activity: failed to submit DestroyActivityItem: {e:#}");
                } else {
                    log::info!("activity: sent DestroyActivityItem to application thread");
                }
            }
        });
    }

    /// Handles ACTION_OPEN_DOCUMENT / ACTION_GET_CONTENT by prompting the user with the native
    /// desktop file picker (zenity) and delivering the selected file as an ActivityResultItem back to the calling activity.
    pub fn open_document(&self, result_to: Option<SIBinder>, result_who: Option<String>, request_code: i32) {
        let Some(result_to) = result_to else {
            log::warn!("activity: open_document with no result_to token");
            return;
        };
        let attached = self.attached.lock().unwrap().clone();
        let Some((thread, _spec)) = attached else {
            log::warn!("activity: open_document with no attached thread");
            return;
        };
        let media_service = self.media_service.clone();

        std::thread::spawn(move || {
            log::info!("activity: spawning native file picker for OPEN_DOCUMENT");
            let output = std::process::Command::new("zenity")
                .arg("--file-selection")
                .arg("--title=Select Audio File")
                .arg("--file-filter=Audio files | *.ogg *.wav *.mp3 *.flac *.m4a *.aac *.opus")
                .output();

            let (result_code, uri_str) = match output {
                Ok(out) if out.status.success() => {
                    let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !path_str.is_empty() {
                        let path = std::path::PathBuf::from(&path_str);
                        log::info!("activity: picked audio file {path_str}");
                        let item = media_service.add_custom_file(path);
                        (-1, Some(format!("content://media/external/audio/media/{}", item.id)))
                    } else {
                        (0, None)
                    }
                }
                Ok(out) => {
                    log::info!("activity: file picker dismissed (exit code {:?})", out.status.code());
                    (0, None)
                }
                Err(e) => {
                    log::warn!("activity: failed to launch zenity: {e}");
                    (0, None)
                }
            };

            let Some(proxy) = thread.as_proxy() else {
                log::error!("activity: IApplicationThread is not a proxy");
                return;
            };

            if let Ok(mut d) = proxy.prepare_transact(true) {
                let _ = d.write_i32(1); // typed object marker (ClientTransaction != null)
                let _ = d.write_i32(1); // 1 item in transaction list
                let _ = ap::string16(&mut d, Some("android.app.servertransaction.ActivityResultItem"));
                // ActivityTransactionItem: mActivityToken
                let _ = d.write(&Some(result_to));
                // dest.writeTypedList(mResultInfoList)
                let _ = d.write_i32(1); // 1 ResultInfo in list
                let _ = d.write_i32(1); // non-null ResultInfo
                // ResultInfo:
                let _ = ap::string16(&mut d, result_who.as_deref());
                let _ = d.write_i32(request_code);
                let _ = d.write_i32(result_code); // RESULT_OK = -1, RESULT_CANCELED = 0
                if result_code == -1 && uri_str.is_some() {
                    let _ = d.write_i32(1); // mData != null
                    let res_intent = crate::parcelables::Intent {
                        action: None,
                        data: uri_str,
                        package: None,
                        component: None,
                        categories: Vec::new(),
                        flags: 1, // Intent.FLAG_GRANT_READ_URI_PERMISSION
                        extras: None,
                    };
                    let _ = res_intent.write(&mut d);
                } else {
                    let _ = d.write_i32(0); // mData == null
                }
                let _ = d.write(&None::<SIBinder>); // mCallerToken = null

                if let Err(e) = proxy.submit_transact(SCHEDULE_TRANSACTION, &d, FLAG_ONEWAY) {
                    log::error!("activity: failed to submit ActivityResultItem: {e:#}");
                } else {
                    log::info!("activity: sent ActivityResultItem (resultCode={result_code}) to application thread");
                }
            }
        });
    }
}

impl Service for ActivityRef {
    const DESCRIPTOR: &'static str = "android.app.IActivityManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYMANAGER;
    fn handle(&self, name: &str, code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        self.0.handle(name, code, data, reply)
    }
}

/// A MAIN/LAUNCHER intent for the app's entry activity.
fn main_intent(package: &str, activity: &str) -> crate::parcelables::Intent {
    crate::parcelables::Intent {
        action: Some("android.intent.action.MAIN".into()),
        data: None,
        package: None,
        component: Some((package.to_string(), activity.to_string())),
        categories: vec!["android.intent.category.LAUNCHER".into()],
        flags: crate::parcelables::FLAG_ACTIVITY_NEW_TASK,
        extras: None,
    }
}

/// The scheme of a URI string (the part before the first ':').
pub fn uri_scheme(uri: &str) -> Option<String> {
    uri.split_once(':').map(|(s, _)| s.to_ascii_lowercase())
}

pub fn infer_mime_type(uri: &str) -> Option<&'static str> {
    if uri.contains("/events/") {
        Some("vnd.android.cursor.item/event")
    } else if uri.contains("/events") {
        Some("vnd.android.cursor.dir/event")
    } else if uri.ends_with(".ogg") {
        Some("audio/ogg")
    } else if uri.ends_with(".mp3") {
        Some("audio/mp3")
    } else if uri.ends_with(".wav") {
        Some("audio/wav")
    } else if uri.ends_with(".png") {
        Some("image/png")
    } else if uri.ends_with(".jpg") || uri.ends_with(".jpeg") {
        Some("image/jpeg")
    } else {
        None
    }
}

pub fn matches_mime(filter_mime: &str, target_mime: &str) -> bool {
    if filter_mime == "*/*" || filter_mime == target_mime {
        return true;
    }
    if let Some((f_major, f_minor)) = filter_mime.split_once('/') {
        if let Some((t_major, _)) = target_mime.split_once('/') {
            if f_minor == "*" && f_major.eq_ignore_ascii_case(t_major) {
                return true;
            }
        }
    }
    false
}

impl ActivityService {
    /// Resolve an implicit intent against a spec's activities.
    pub fn resolve(spec: &AppSpec, action: &str, data: Option<&str>) -> Option<String> {
        let scheme = data.and_then(uri_scheme);
        let mime = data.and_then(infer_mime_type);
        spec.activities.iter().find(|a| {
            a.filters.iter().any(|f| {
                if !f.actions.is_empty() && !f.actions.iter().any(|x| x == action) {
                    return false;
                }
                if !f.categories.is_empty() && !f.categories.iter().any(|c| c == "android.intent.category.DEFAULT") {
                    return false;
                }
                match (&scheme, &mime) {
                    (Some(s), Some(m)) => {
                        let scheme_matches = if f.schemes.is_empty() {
                            s == "content" || s == "file"
                        } else {
                            f.schemes.iter().any(|fs| fs == s)
                        };
                        let mime_matches = f.mime_types.is_empty() || f.mime_types.iter().any(|fm| matches_mime(fm, m));
                        scheme_matches && mime_matches
                    }
                    (Some(s), None) => {
                        if f.schemes.is_empty() {
                            f.mime_types.is_empty() && (s == "content" || s == "file")
                        } else {
                            f.schemes.iter().any(|fs| fs == s)
                        }
                    }
                    (None, Some(m)) => {
                        f.mime_types.iter().any(|fm| matches_mime(fm, m))
                    }
                    (None, None) => {
                        f.schemes.is_empty() && f.mime_types.is_empty()
                    }
                }
            })
        }).map(|a| a.name.clone())
    }

    /// Dispatch a startActivity: resolve the target and launch it into the
    /// running app. Explicit (component set) same-app intents only for now.
    pub fn start_activity(&self, pkg: Option<String>, class: Option<String>, action: Option<String>, data: Option<String>) {
        self.start_activity_target(crate::pending_intent::Target {
            package: pkg,
            class,
            action,
            data,
            ..Default::default()
        });
    }

    pub fn start_activity_target(&self, target_intent: crate::pending_intent::Target) {
        let attached = self.attached.lock().unwrap().clone();
        let controller = self.client_controller.lock().unwrap().clone();
        let display = self.registry.display;
        let (Some((thread, spec)), Some(controller)) = (attached, controller) else {
            log::warn!("activity: startActivity with no running app");
            return;
        };
        let target = match (target_intent.package.as_deref(), target_intent.class.clone()) {
            (Some(p), Some(c)) if p == spec.package => c,
            (Some(p), None) if p == spec.package => spec.main_activity.clone().unwrap_or_default(),
            (None, Some(c)) => c,
            (Some(p), Some(_)) => {
                log::warn!("activity: startActivity to another package {p:?} not supported yet");
                return;
            }
            _ => {
                // Implicit intent: match this app's activities' <intent-filter>s by action.
                let Some(act) = target_intent.action.as_deref() else {
                    log::warn!("activity: implicit startActivity with no action");
                    return;
                };
                match Self::resolve(&spec, act, target_intent.data.as_deref()) {
                    Some(a) => {
                        log::info!("activity: implicit {act:?} resolved to {a}");
                        a
                    }
                    None => {
                        log::warn!("activity: no activity matches implicit action {act:?}");
                        return;
                    }
                }
            }
        };
        let target = spec.activities.iter()
            .find(|a| a.name == target)
            .and_then(|a| a.target_activity.clone())
            .unwrap_or(target);
        if !spec.activities.iter().any(|a| a.name == target) {
            log::warn!("activity: startActivity target {target} not declared");
            return;
        }

        let mut stack = self.activity_stack.lock().unwrap();
        if stack.contains(&target) {
            log::info!("activity: target {target} is already running in task");
            drop(stack);
            return;
        }
        stack.push(target.clone());
        drop(stack);

        let intent = crate::parcelables::Intent {
            action: target_intent.action,
            data: target_intent.data,
            package: None,
            component: Some((spec.package.clone(), target.clone())),
            categories: target_intent.categories,
            flags: crate::parcelables::FLAG_ACTIVITY_NEW_TASK,
            extras: target_intent.extras,
        };
        log::info!("activity: startActivity -> {}/{target} (extras={})", spec.package, intent.extras.is_some());
        std::thread::spawn(move || {
            if let Err(e) = Self::launch_activity(&thread, &spec, &controller, display, &target, intent) {
                log::error!("activity: startActivity launch failed: {e:#}");
            }
        });
    }


    pub fn start_service(&self, target: &crate::pending_intent::Target) {
        let attached = self.attached.lock().unwrap().clone();
        let (Some((thread, spec)), Some(target_class)) = (attached, target.class.as_deref()) else {
            log::warn!("activity: start_service cannot dispatch (attached or class missing): {target:?}");
            return;
        };
        let proxy = match thread.as_proxy() {
            Some(p) => p,
            None => {
                log::error!("activity: IApplicationThread is not a proxy");
                return;
            }
        };

        let mut services = self.running_services.lock().unwrap();
        let (token, is_new) = match services.get(target_class) {
            Some(tok) => (tok.clone(), false),
            None => {
                let tok = super::token::new_token("service");
                services.insert(target_class.to_string(), tok.clone());
                (tok, true)
            }
        };
        drop(services);

        let service_info = crate::parcelables::ServiceInfo {
            name: target_class.to_string(),
            package_name: spec.package.clone(),
            application_info: Registry::application_info(&spec),
            process_name: spec.package.clone(),
            permission: None,
            flags: 0,
            foreground_service_type: 0,
        };

        if is_new {
            log::info!("activity: scheduleCreateService for {target_class}");
            let mut d = match proxy.prepare_transact(true) {
                Ok(d) => d,
                Err(e) => {
                    log::error!("activity: prepare_transact scheduleCreateService failed: {e:#}");
                    return;
                }
            };
            if let Err(e) = (|| -> Result<()> {
                d.write(&Some(token.clone()))?;
                ap::typed(&mut d, |p| service_info.write(p))?;
                ap::typed(&mut d, CompatibilityInfo::write_default)?;
                d.write_i32(4)?; // processState
                Ok(())
            })() {
                log::error!("activity: marshalling scheduleCreateService failed: {e:#}");
                return;
            }
            if let Err(e) = proxy.submit_transact(SCHEDULE_CREATE_SERVICE, &d, FLAG_ONEWAY) {
                log::error!("activity: scheduleCreateService failed: {e:#}");
                return;
            }
        }

        let start_id = self.next_service_start_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let intent = target.to_intent();
        log::info!("activity: scheduleServiceArgs for {target_class} (start_id={start_id}, action={:?})", intent.action);

        let mut d = match proxy.prepare_transact(true) {
            Ok(d) => d,
            Err(e) => {
                log::error!("activity: prepare_transact scheduleServiceArgs failed: {e:#}");
                return;
            }
        };
        if let Err(e) = (|| -> Result<()> {
            d.write(&Some(token))?;
            ap::typed(&mut d, |p| {
                // ParceledListSlice<ServiceStartArgs>
                p.write_i32(1)?; // count = 1
                ap::string16(p, Some("android.app.ServiceStartArgs"))?;
                p.write_i32(1)?; // element 0 inline
                p.write_i32(0)?; // taskRemoved = 0
                p.write_i32(start_id)?; // startId
                p.write_i32(0)?; // flags = 0
                p.write_i32(1)?; // intent non-null
                intent.write(p)
            })?;
            Ok(())
        })() {
            log::error!("activity: marshalling scheduleServiceArgs failed: {e:#}");
            return;
        }

        if let Err(e) = proxy.submit_transact(SCHEDULE_SERVICE_ARGS, &d, FLAG_ONEWAY) {
            log::error!("activity: scheduleServiceArgs failed: {e:#}");
        }
    }

    pub fn stop_service(&self, token: &SIBinder) {
        let mut services = self.running_services.lock().unwrap();
        services.retain(|k, v| {
            if v == token {
                log::info!("activity: service removed from running_services: {k}");
                false
            } else {
                true
            }
        });
        if let Some((thread, _)) = self.attached.lock().unwrap().clone() {
            if let Some(proxy) = thread.as_proxy() {
                if let Ok(mut d) = proxy.prepare_transact(true) {
                    let _ = d.write(&Some(token.clone()));
                    let _ = proxy.submit_transact(SCHEDULE_STOP_SERVICE, &d, FLAG_ONEWAY);
                }
            }
        }
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
                    let (disabled_compat, enabled_compat) = self.compat.compute_compat_changes(spec.target_sdk);
                    log::info!("compat: computed {} disabled, {} enabled changes for {} (target sdk {})",
                        disabled_compat.len(), enabled_compat.len(), spec.package, spec.target_sdk);
                    std::thread::spawn(move || {
                        if let Err(e) = Self::bind_application(&thread, &spec, display, &disabled_compat, &enabled_compat) {
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
                    let url = self.launch_url.lock().unwrap().clone();
                    let (target, intent) = if let Some(uri) = url {
                        // Deep link: use requested_activity if specified, else resolve against intent-filters.
                        let chosen = spec.requested_activity.clone().or_else(|| Self::resolve(&spec, "android.intent.action.VIEW", Some(&uri)));
                        match chosen {
                            Some(cls) => {
                                log::info!("activity: deep link {uri:?} -> {cls}");
                                (cls.clone(), crate::parcelables::Intent {
                                    action: Some("android.intent.action.VIEW".into()),
                                    data: Some(uri),
                                    package: None,
                                    component: Some((spec.package.clone(), cls)),
                                    categories: vec![],
                                    flags: crate::parcelables::FLAG_ACTIVITY_NEW_TASK,
                                    extras: None,
                                })
                            }
                            None => {
                                log::warn!("activity: no activity handles {uri:?}; falling back to launcher");
                                let main = spec.main_activity.clone().unwrap_or_default();
                                (main.clone(), main_intent(&spec.package, &main))
                            }
                        }
                    } else {
                        let Some(main) = spec.main_activity.clone() else {
                            log::error!("activity: no main activity for {}", spec.package);
                            return Ok(true);
                        };
                        (main.clone(), main_intent(&spec.package, &main))
                    };
                    let target = spec.activities.iter()
                        .find(|a| a.name == target)
                        .and_then(|a| a.target_activity.clone())
                        .unwrap_or(target);
                    self.activity_stack.lock().unwrap().push(target.clone());
                    std::thread::spawn(move || match Self::launch_activity(&thread, &spec, &controller, display, &target, intent) {
                        Ok(()) => log::info!("activity: launch transaction sent for {}", spec.package),
                        Err(e) => log::error!("activity: launch failed: {e:#}"),
                    });
                }
                Ok(true)
            }
            "getIntentSender" | "getIntentSenderWithFeature" => {
                // (int type, String pkg, [String featureId], IBinder token, String resultWho,
                //  int requestCode, Intent[] intents, String[] resolvedTypes, int flags, Bundle, int user)
                let _type = data.read_i32()?;
                let _pkg: Option<String> = data.read()?;
                if name == "getIntentSenderWithFeature" {
                    let _feature: Option<String> = data.read()?;
                }
                let _token: Option<SIBinder> = data.read()?;
                let _result_who: Option<String> = data.read()?;
                let _request_code = data.read_i32()?;
                let count = data.read_i32()?;
                let mut target = crate::pending_intent::Target::default();
                if count >= 1 && data.read_i32()? != 0 {
                    target = crate::pending_intent::read_intent_target(data)?; // first intent's target
                }
                target.intent_type = _type;
                log::info!("activity: getIntentSender type={_type} -> {target:?}");
                let sender = self.pending_intents.create(target);
                ap::no_exception(reply)?;
                reply.write(&Some(sender))?;
                Ok(true)
            }
            "getContentProvider" => {
                let _caller: Option<SIBinder> = data.read().ok().flatten();
                let _calling_pkg: Option<String> = data.read().ok().flatten();
                let name: Option<String> = data.read().ok().flatten();
                let _user_id = data.read_i32().unwrap_or(0);
                let _stable = data.read_i32().unwrap_or(0) != 0;
                log::info!("activity: getContentProvider name={name:?}");
                ap::no_exception(reply)?;
                if name.as_deref() == Some("settings") {
                    let token = super::token::new_token("settings_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.settings.SettingsProvider".into(),
                            package_name: "com.android.providers.settings".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "system".into(),
                            exported: true,
                            authority: "settings".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.settings_provider.clone(),
                        connection: token,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else if name.as_deref() == Some("media") {
                    let token = super::token::new_token("media_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.media.MediaProvider".into(),
                            package_name: "com.android.providers.media.module".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "android.process.media".into(),
                            exported: true,
                            authority: "media".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.media_provider.clone(),
                        connection: token,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else if name.as_deref() == Some("com.android.calendar") {
                    let token = super::token::new_token("calendar_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.calendar.CalendarProvider2".into(),
                            package_name: "com.android.providers.calendar".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "android.process.calendar".into(),
                            exported: true,
                            authority: "com.android.calendar".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.calendar_provider.clone(),
                        connection: token,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getContentProviderExternal" => {
                let name: Option<String> = data.read().ok().flatten();
                let _user_id = data.read_i32().unwrap_or(0);
                let _token: Option<SIBinder> = data.read().ok().flatten();
                let _tag: Option<String> = data.read().ok().flatten();
                log::info!("activity: getContentProviderExternal name={name:?}");
                ap::no_exception(reply)?;
                if name.as_deref() == Some("settings") {
                    let conn = super::token::new_token("settings_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.settings.SettingsProvider".into(),
                            package_name: "com.android.providers.settings".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "system".into(),
                            exported: true,
                            authority: "settings".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.settings_provider.clone(),
                        connection: conn,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else if name.as_deref() == Some("media") {
                    let conn = super::token::new_token("media_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.media.MediaProvider".into(),
                            package_name: "com.android.providers.media.module".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "android.process.media".into(),
                            exported: true,
                            authority: "media".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.media_provider.clone(),
                        connection: conn,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else if name.as_deref() == Some("com.android.calendar") {
                    let conn = super::token::new_token("calendar_provider_conn");
                    let holder = crate::parcelables::ContentProviderHolder {
                        info: crate::parcelables::ProviderInfo {
                            name: "com.android.providers.calendar.CalendarProvider2".into(),
                            package_name: "com.android.providers.calendar".into(),
                            application_info: crate::parcelables::ApplicationInfo::default(),
                            process_name: "android.process.calendar".into(),
                            exported: true,
                            authority: "com.android.calendar".into(),
                            read_permission: None,
                            write_permission: None,
                            grant_uri_permissions: true,
                            multiprocess: false,
                            init_order: 100,
                            syncable: false,
                        },
                        provider: self.calendar_provider.clone(),
                        connection: conn,
                        no_release_needed: true,
                        local: false,
                    };
                    ap::typed(reply, |p| holder.write(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "refContentProvider" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "removeContentProvider"
            | "removeContentProviderExternal"
            | "removeContentProviderExternalAsUser" => Ok(true), // oneway
            "registerReceiverWithFeature" | "registerReceiver" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "unregisterReceiver" => {
                ap::no_exception(reply)?;
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
            "startService" => {
                let _caller: Option<SIBinder> = data.read()?;
                let has_intent = data.read_i32()?;
                let mut target = crate::pending_intent::Target::default();
                if has_intent != 0 {
                    target = crate::pending_intent::read_intent_target(data)?;
                }
                target.intent_type = 4; // INTENT_SENDER_SERVICE
                let _resolved_type: Option<String> = data.read()?;
                let _require_fg = data.read_i32()?;
                let _calling_pkg: Option<String> = data.read()?;
                let _feature: Option<String> = data.read()?;
                let _user = data.read_i32()?;
                log::info!("activity: startService -> {target:?}");
                self.start_service(&target);
                ap::no_exception(reply)?;
                ap::typed(reply, |p| crate::parcelables::write_component_name(p, target.package.as_deref(), target.class.as_deref()))?;
                Ok(true)
            }
            "stopServiceToken" => {
                let has_comp = data.read_i32()?;
                if has_comp != 0 {
                    let _pkg: Option<String> = data.read()?;
                    let _cls: Option<String> = data.read()?;
                }
                let token: Option<SIBinder> = data.read()?;
                let _start_id = data.read_i32()?;
                if let Some(ref tok) = token {
                    self.stop_service(tok);
                }
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "serviceDoneExecuting" => {
                let _token: Option<SIBinder> = data.read()?;
                let _type = data.read_i32()?;
                let _start_id = data.read_i32()?;
                let _res = data.read_i32()?;
                ap::no_exception(reply)?;
                Ok(true)
            }
            "cancelIntentSender" => {
                let _sender: Option<SIBinder> = data.read()?;
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

