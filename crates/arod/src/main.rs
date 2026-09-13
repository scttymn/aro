//! arod: the ARO session supervisor.
//!
//! Creates the session namespaces, mounts the private binderfs, becomes the
//! Binder context manager, hosts the native services, sinks Android logs,
//! and launches app processes through `aro-exec`.
mod pending_intent;
mod aparcel;
mod hub;
mod parcelables;
mod services;
mod session;
mod shm;

use anyhow::{bail, Context, Result};
mod bundle;
mod dnsproxy;
mod hostnet;
mod portal;
mod location_setup;
mod host_services;
mod shade;
mod compositor;
#[allow(dead_code)]
mod notify;
mod input_channel;
mod keyboard;
use aro_exec::{layout::Layout, logd, ns::Session, prepare};
use clap::{Parser, Subcommand};
use rsbinder::hub::android_16::android::os::IServiceManager::BnServiceManager;
use rsbinder::ProcessState;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "arod", about = "ARO session supervisor")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Internal native Binder worker.
    #[command(hide = true)]
    HostService {
        #[arg(value_enum)]
        kind: host_services::Kind,
        #[arg(long)]
        binder: PathBuf,
        #[arg(long)]
        host_uid: u32,
        #[arg(long)]
        supervisor_pid: i32,
    },
    /// Internal host-side first-use location helper (private stdio protocol).
    #[command(hide = true)]
    LocationHost,
    /// Start a session, then run the M1 bootstrap against an APK inside it
    Run {
        apk: PathBuf,
        class: String,
        method: Option<String>,
    },
    /// Start a session and run a shell inside it
    Shell,
    /// Register a desktop scheme handler (.desktop + xdg-mime) so the desktop
    /// routes an app's deep-link URLs (its VIEW intent-filter schemes) to ARO.
    DesktopEntry {
        apk: PathBuf,
    },
    /// Start a session and launch an APK as a real app process (ActivityThread.main)
    App {
        apk: PathBuf,
        /// Activity to launch (default: the manifest's LAUNCHER activity)
        #[arg(long)]
        activity: Option<String>,
        /// Deep link: launch via ACTION_VIEW with this URI instead of MAIN.
        #[arg(long)]
        url: Option<String>,
    },
    /// Copy an APK into ARO storage and register its desktop launcher
    Install { apk: PathBuf },
    /// Pick an APK with the desktop portal and install it
    Pick,
    /// Inbound intent bridge: open a URI or host file in the matching ARO app
    Open {
        uri: String,
    },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).format_timestamp_millis().init();
    let cli = Cli::parse();
    if let Cmd::HostService { kind, binder, host_uid, supervisor_pid } = &cli.cmd {
        return host_services::serve(*kind, binder, *host_uid, *supervisor_pid);
    }
    if let Cmd::LocationHost = &cli.cmd { return location_setup::serve(); }
    // A desktop-entry request only writes host files; it needs no session or image.
    if let Cmd::DesktopEntry { apk } = &cli.cmd {
        return write_desktop_entry(apk);
    }
    if let Cmd::Open { uri } = &cli.cmd {
        return open_uri(uri);
    }
    if let Cmd::Install { apk } = &cli.cmd { return install_apk(apk); }
    if let Cmd::Pick = &cli.cmd {
        if let Some(apk) = portal::open_file(nix::unistd::getuid().as_raw(), "ARO: Install Android app", Some("application/vnd.android.package-archive"))? { install_apk(&apk)?; }
        return Ok(());
    }
    let layout = Layout::default();
    if !layout.system.join("system/bin/app_process64").exists() {
        bail!("no unpacked system image at {} (run: aro-image unpack)", layout.system.display());
    }

    // Keep permission UI and package installation on the real host. Starting a
    // child does not create supervisor threads before entering the namespaces.
    let location_setup = match location_setup::HostSetup::spawn() {
        Ok(helper) => Some(std::sync::Arc::new(helper)),
        Err(e) => { log::warn!("location setup unavailable: {e:#}"); None }
    };
    // 1. Session namespaces (single-threaded here).
    let session = session::enter(&layout)?;
    prepare::prepare(&layout)?;

    // 2. Log sink.
    let sock = logd::bind(&session.sockets)?;
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        loop {
            logd::drain_blocking(&sock, &mut buf);
        }
    });

    // 3. Binder bus: become the context manager on the session's binder.
    let binder_path = session.binderfs.join("binder");
    ProcessState::init(binder_path.to_str().context("binder path")?, 0).map_err(|e| anyhow::anyhow!("binder init: {e}"))?;
    let hub_impl = std::sync::Arc::new(hub::Hub::default());
    let hub = BnServiceManager::new_binder(hub::HubRef(hub_impl.clone()));
    ProcessState::as_self().become_context_manager(hub.as_binder()).map_err(|e| anyhow::anyhow!("become context manager: {e}"))?;
    ProcessState::start_thread_pool();
    log::info!("arod: bus up on {}", binder_path.display());

    // 3a. The host: which display, at what scale and refresh, and which app we
    // are about to show. Everything below sizes itself from these; nothing is
    // assumed about the desktop.
    let host = compositor::connect();
    let manifest = match &cli.cmd {
        Cmd::App { apk, .. } => Some(aro_apk::inspect(&apk.canonicalize()?)?),
        _ => None,
    };
    let (hd, host_conn) = match host {
        Some(h) => (h.display.clone(), Some(h)),
        None => {
            if manifest.is_some() {
                bail!("no Wayland display (WAYLAND_DISPLAY unset or no compositor); an app needs one to be shown");
            }
            // Headless (run/shell only): a placeholder so services can be published; no window exists.
            (compositor::HostDisplay { name: "headless".into(), width: 0, height: 0, scale: 1, refresh_mhz: 0 }, None)
        }
    };
    let display = hd.tuple();

    // 3b. Services.
    let registry = std::sync::Arc::new(services::registry::Registry { apps: std::sync::Mutex::new(Vec::new()), display });
    let pending_intents = pending_intent::Registry::new();
    let compat = std::sync::Arc::new(services::compat::PlatformCompatService::load(&layout.system, registry.clone()));
    let workers = host_services::Workers::new(hub_impl.clone(), binder_path.clone(), session.host_uid);
    workers.spawn(host_services::Kind::SettingsProvider, None)?;
    let settings_provider = workers.service(host_services::SETTINGS_PROVIDER)?;
    workers.spawn(host_services::Kind::Audio, None)?;
    workers.spawn(host_services::Kind::MediaProvider, None)?;
    let media_provider = workers.service(host_services::MEDIA_PROVIDER)?;
    let documents = workers.service(host_services::DOCUMENTS)?;
    workers.spawn(host_services::Kind::CalendarProvider, None)?;
    let calendar_provider = workers.service(host_services::CALENDAR_PROVIDER)?;
    let activity = std::sync::Arc::new(services::activity::ActivityService {
        registry: registry.clone(),
        host_uid: session.host_uid,
        compat: compat.clone(),
        settings_provider,
        media_provider,
        documents,
        calendar_provider,
        pending: std::sync::Mutex::new(None),
        service_processes: services::service_processes::ServiceProcesses::default(),
        attached: std::sync::Mutex::new(None),
        client_controller: std::sync::Mutex::new(None),
        launch_url: std::sync::Mutex::new(None),
        pending_intents: pending_intents.clone(),
        activity_stack: std::sync::Mutex::new(Vec::new()),
        running_services: std::sync::Mutex::new(std::collections::HashMap::new()),
        bindings: std::sync::Mutex::new(Vec::new()),
        next_service_start_id: std::sync::atomic::AtomicI32::new(1),
    });
    let controller = services::binder_of(services::activity_task::ActivityClientController { activity: activity.clone() });
    *activity.client_controller.lock().unwrap() = Some(controller.clone());
    services::publish(&hub_impl, "activity_task", services::activity_task::ActivityTaskService { client_controller: std::sync::Mutex::new(Some(controller)), activity: activity.clone() });
    // Gralloc (allocator + mapper) — created early so the composer can resolve
    // posted buffers to their memfds for the Wayland presenter.
    let gralloc = std::sync::Arc::new(services::allocator::Gralloc::default());
    let input_hub = std::sync::Arc::new(input_channel::InputHub::default());
    // Composer + window manager.
    let sf = std::sync::Arc::new(services::surfaceflinger::SurfaceFlinger::new(hd.width as u32, hd.height as u32, hd.dpi() as u32, hd.frame_interval_ns()));
    let composer_client_host = std::sync::Arc::new(std::sync::Mutex::new(None));
    let composer_client = services::binder_of(services::surfaceflinger::ComposerClient { sf: sf.clone(), host: composer_client_host.clone() });
    *sf.client.lock().unwrap() = Some(composer_client.clone());
    let display_token = services::token::new_token("display");
    services::publish(&hub_impl, "SurfaceFlingerAIDL", services::surfaceflinger::ComposerAidl { sf: sf.clone(), client: composer_client, display_token });
    let window_host = std::sync::Arc::new(services::window_session::WindowHost::new(sf.clone(), (hd.width, hd.height), hd.dpi(), hd.scale, input_hub.clone()));
    *composer_client_host.lock().unwrap() = Some(window_host.clone());
    // Window identity is the app's: package as app_id (desktop matches it to a
    // .desktop entry); the title is the package until the manifest label is
    // resolved from resources.arsc.
    let package = manifest.as_ref().map(|m| m.package.clone()).unwrap_or_else(|| "aro".to_string());
    let presenter = host_conn.and_then(|h| compositor::Presenter::spawn(h, package.clone(), package.clone(), input_hub.clone(), window_host.clone()));
    services::publish(&hub_impl, "SurfaceFlinger", services::surfaceflinger::ComposerLegacy { sf: sf.clone(), transactions: std::sync::atomic::AtomicU32::new(0), gralloc: gralloc.clone(), presenter });
    let window_session = services::binder_of(services::window_session::WindowSession { host: window_host.clone() });
    services::publish(&hub_impl, "window", services::window::WindowService { session: window_session });
    services::publish(&hub_impl, "input_method", services::input_method::InputMethodService);
    services::publish(&hub_impl, "input", services::input::InputService);
    // Notifications go to whatever owns org.freedesktop.Notifications on the session bus.
    // The launched app's launcher icon, extracted from the APK and written to
    // the runtime icons dir, for the shade avatar. None → the widget shows a
    // generic Android badge instead.
    let app_icon: Option<String> = match &cli.cmd {
        Cmd::App { apk, .. } => aro_apk::extract_icon(&apk.canonicalize().unwrap_or_else(|_| apk.clone())).and_then(|(bytes, ext)| {
            let dir = layout.runtime.join("icons");
            std::fs::create_dir_all(&dir).ok()?;
            let path = dir.join(format!("{package}.{ext}"));
            std::fs::write(&path, &bytes).ok()?;
            Some(path.to_string_lossy().into_owned())
        }),
        _ => None,
    };

    let fire_activity = activity.clone();
    let fire: std::sync::Arc<dyn Fn(&pending_intent::Target) + Send + Sync> =
        std::sync::Arc::new(move |t: &pending_intent::Target| {
            log::info!("intent dispatch: target -> {t:?}");
            if t.intent_type == 4 || t.intent_type == 5 || (t.intent_type == 0 && t.class.as_deref().map(|c| c.contains("Service")).unwrap_or(false)) {
                fire_activity.start_service(t);
            } else {
                fire_activity.start_activity(t.package.clone(), t.class.clone(), t.action.clone(), t.data.clone());
            }
        });
    pending_intents.set_fire_handler(fire.clone());
    // The ARO shade: an Android notification list served to the companion Omarchy
    // widget on the top bar over a Unix socket.
    let shade = shade::Shade::new(fire);
    if let Err(e) = shade.serve(&layout.runtime.join("notifications.sock")) {
        log::warn!("shade: not serving ({e}); companion widget will be empty");
    }
    services::publish(&hub_impl, "notification", services::notification::NotificationService { pending_intents: pending_intents.clone(), shade: shade.clone(), app_icon });
    services::publish(&hub_impl, "contextual_mode", services::modes::ModesService);
    // Network: mirror the host's connection (NetworkManager on the system bus).
    if let Err(e) = dnsproxy::serve(&session.sockets) { log::warn!("dnsproxyd: {e}"); }
    let location_worker_pid = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    services::publish(&hub_impl, host_services::LOCATION_AUTHORITY, host_services::LocationAuthority {
        activity: std::sync::Arc::downgrade(&activity), worker_pid: location_worker_pid.clone(),
    });
    if let Some(setup) = location_setup {
        let pid = workers.spawn(host_services::Kind::Location, Some(setup))?;
        location_worker_pid.store(pid, std::sync::atomic::Ordering::Release);
    }
    let hostnet = hostnet::probe(session.host_uid);
    workers.spawn(host_services::Kind::Connectivity, None)?;
    // The allocator is a VINTF-stable HAL binder; the mapper half is a bionic
    // library bound into the app at /vendor/lib64/hw/mapper.aro.so.
    hub_impl.register("android.hardware.graphics.allocator.IAllocator/default", services::vintf_binder_of(services::allocator::AllocatorService { gralloc: gralloc.clone() }));
    let vendor_dir = prepare_vendor_dir(&layout)?;
    services::publish(&hub_impl, "accessibility", services::accessibility::AccessibilityService);
    services::publish(&hub_impl, "user", services::user::UserService);
    services::publish(&hub_impl, "alarm", services::alarm::AlarmService::new(activity.clone(), pending_intents.clone()));
    services::publish(&hub_impl, "shortcut", services::shortcut::ShortcutService);
    services::publish(&hub_impl, "appops", services::appops::AppOpsService);
    services::publish(&hub_impl, "uimode", services::uimode::UiModeService);
    services::publish(&hub_impl, "power", services::power::PowerService);
    services::publish(&hub_impl, "thermalservice", services::thermal::ThermalService);
    workers.spawn(host_services::Kind::Clipboard, None)?;
    services::publish(&hub_impl, "content", services::content::ContentService);
    workers.spawn(host_services::Kind::Storage, None)?;
    services::publish(&hub_impl, "sensorservice", services::sensor::SensorService);
    services::publish(&hub_impl, "media.camera", services::camera::CameraService);
    workers.spawn(host_services::Kind::MediaPlayer, None)?;
    workers.spawn(host_services::Kind::AudioFlinger, None)?;
    workers.spawn(host_services::Kind::AudioPolicy, None)?;
    services::publish(&hub_impl, "package", services::package::PackageService { registry: registry.clone() });
    services::publish(&hub_impl, "platform_compat", services::compat::PlatformCompatRef(compat.clone()));
    services::publish(&hub_impl, "display", services::display::DisplayService { name: hd.name.clone(), width: hd.width, height: hd.height, dpi: hd.dpi(), callbacks: std::sync::Mutex::new(Vec::new()) });
    services::publish(&hub_impl, "activity", services::activity::ActivityRef(activity.clone()));
    services::publish(&hub_impl, "uri_grants", services::uri_grants::UriGrantsService);
    services::publish(&hub_impl, "webviewupdate", services::webview_update::WebViewUpdateService { registry: registry.clone() });

    // Properties are cheap to regenerate and ARO's extra ones evolve with the runtime.
    let mut extra_props: Vec<(String, String)> = Vec::new();
    for (i, dns) in hostnet.dns.iter().enumerate() {
        extra_props.push((format!("net.dns{}", i + 1), dns.to_string()));
    }
    let n = prepare::write_properties(&layout.system, &layout.state, &extra_props)?;
    log::info!("arod: {n} system properties");

    // 4. Launch.
    let exe = std::env::current_exe()?.with_file_name("aro-exec");
    let mut cmd = std::process::Command::new(&exe);
    // The ART launcher belongs to this supervisor just like the service workers.
    // Its own parent-death handling then tears down the Android PID namespace.
    let supervisor_pid = unsafe { libc::getpid() };
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != supervisor_pid {
                return Err(std::io::Error::other("supervisor exited before app launch"));
            }
            Ok(())
        });
    }
    cmd.env(Session::ENV_BINDERFS, &session.binderfs).env(Session::ENV_SOCKETS, &session.sockets);
    if let Some(v) = &vendor_dir {
        cmd.env("ARO_VENDOR_DIR", v);
    }
    // External storage maps to a host directory: ARO_SDCARD, else the user's home.
    let sdcard = std::env::var_os("ARO_SDCARD").or_else(|| std::env::var_os("HOME"));
    if let Some(ref sd) = sdcard {
        cmd.env("ARO_SDCARD", sd);
    }
    *activity.service_processes.launcher.lock().unwrap() = Some(services::service_processes::Launcher {
        exe: exe.clone(),
        env: cmd.get_envs().filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string()))).collect(),
    });
    match cli.cmd {
        Cmd::Run { apk, class, method } => {
            cmd.arg("run").arg(apk).arg(class);
            if let Some(m) = method {
                cmd.arg(m);
            }
        }
        Cmd::Shell => {
            cmd.arg("shell");
        }
        Cmd::HostService { .. } | Cmd::LocationHost | Cmd::DesktopEntry { .. } | Cmd::Open { .. } | Cmd::Install { .. } | Cmd::Pick => unreachable!("handled before session setup"),
        Cmd::App { apk, activity: main_activity, url } => {
            let apk = apk.canonicalize()?;
            let name = apk.file_name().unwrap().to_string_lossy().into_owned();
            let manifest = manifest.expect("manifest parsed above");
            let package = manifest.package.clone();
            log::info!("arod: {} v{} target sdk {} app {:?} launcher {:?}", package, manifest.version_code, manifest.target_sdk, manifest.app_class, manifest.main_activity().map(|a| &a.name));
            for d in ["data", "user_de/0"] {
                let base = layout.data.join(d).join(&package);
                std::fs::create_dir_all(&base)?;
                for sub in ["cache", "code_cache", "files", "databases", "shared_prefs"] {
                    let _ = std::fs::create_dir_all(base.join(sub));
                }
            }
            if let Some(sd) = &sdcard {
                let sd_path = std::path::Path::new(sd);
                let _ = std::fs::create_dir_all(sd_path.join("Android/data").join(&package).join("cache"));
                let _ = std::fs::create_dir_all(sd_path.join("Android/data").join(&package).join("files"));
            }
            let user0 = layout.data.join("user/0");
            if !user0.exists() {
                std::fs::create_dir_all(layout.data.join("user"))?;
                std::os::unix::fs::symlink("../data", &user0)?;
            }
            let mut spec = services::registry::AppSpec::from_manifest(&manifest, format!("/data/local/tmp/{name}"), 10001);
            if let Some(a) = main_activity {
                spec.main_activity = Some(a.clone());
                spec.requested_activity = Some(a);
            }
            if let Some(u) = url {
                let u = android_file_uri(&u, sdcard.as_deref().map(std::path::Path::new));
                *activity.launch_url.lock().unwrap() = Some(u);
            }
            let webview_apk = layout.system.join("system/product/app/webview/webview.apk");
            if webview_apk.is_file() {
                if let Ok(manifest) = aro_apk::inspect(&webview_apk) {
                    let webview_spec = services::registry::AppSpec::from_manifest(&manifest, "/system/product/app/webview/webview.apk".into(), 10002);
                    registry.apps.lock().unwrap().push(webview_spec);
                }
            }
            registry.apps.lock().unwrap().push(spec.clone());
            *activity.pending.lock().unwrap() = Some(spec);
            cmd.arg("run").arg("--app").arg(&apk);
        }
    }
    let status = cmd.status().with_context(|| format!("spawning {}", exe.display()))?;
    log::info!("arod: app exited with {status}");
    drop(workers);
    std::process::exit(status.code().unwrap_or(1));
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dst_path)?;
        } else {
            std::fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

/// Assemble the app's /vendor: `lib64/hw/mapper.aro.so` (built from crates/aro-mapper
/// for x86_64-linux-android) and Vulkan HAL driver with supporting libs.
fn prepare_vendor_dir(layout: &aro_exec::layout::Layout) -> anyhow::Result<Option<std::path::PathBuf>> {
    let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let vendor = layout.runtime.join("vendor");
    let hw = vendor.join("lib64").join("hw");
    std::fs::create_dir_all(&hw)?;

    // Copy repo/installed vendor tree if available (contains vulkan.aro.so, libdrm, libexpat, etc.)
    let vendor_sources = [
        std::env::var_os("ARO_VENDOR_SRC").map(std::path::PathBuf::from),
        exe_dir.parent().and_then(|p| p.parent()).map(|p| p.join("vendor")),
        Some(std::path::PathBuf::from("vendor")),
        Some(exe_dir.join("vendor")),
    ];
    if let Some(src_dir) = vendor_sources.into_iter().flatten().find(|p| p.is_dir()) {
        log::info!("vendor: copying vendor libraries from {}", src_dir.display());
        if let Err(e) = copy_dir_all(&src_dir, &vendor) {
            log::warn!("vendor: failed to copy from {}: {e}", src_dir.display());
        }
    }

    // Also symlink/copy vendor/lib64/* into vendor/lib64/hw/ to guarantee loader finds them
    let lib64 = vendor.join("lib64");
    if let Ok(entries) = std::fs::read_dir(&lib64) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_file() {
                    let target = hw.join(entry.file_name());
                    if !target.exists() {
                        let _ = std::fs::copy(entry.path(), &target);
                    }
                }
            }
        }
    }

    let candidates = [
        std::env::var_os("ARO_MAPPER_SO").map(std::path::PathBuf::from),
        Some(exe_dir.join("mapper.aro.so")),
        exe_dir.parent().map(|t| t.join("x86_64-linux-android").join("debug").join("libaro_mapper.so")),
        exe_dir.parent().map(|t| t.join("x86_64-linux-android").join("release").join("libaro_mapper.so")),
    ];
    if let Some(src) = candidates.into_iter().flatten().find(|p| p.is_file()) {
        std::fs::copy(&src, hw.join("mapper.aro.so"))?;
        log::info!("gralloc: mapper {} -> {}", src.display(), hw.join("mapper.aro.so").display());
    } else {
        log::warn!("gralloc: mapper.aro.so not found (build with `cargo build -p aro-mapper --target x86_64-linux-android`); apps cannot allocate buffers");
    }

    Ok(Some(vendor))
}


/// Generate a freedesktop `.desktop` launcher entry for an app and optionally
/// register its deep-link URL scheme handlers. The app appears in the desktop
/// launcher (rofi, wofi, application menu) with its real icon.
fn write_desktop_entry(apk: &std::path::Path) -> Result<()> {
    let apk = apk.canonicalize()?;
    let manifest = aro_apk::inspect(&apk)?;
    let arod = std::env::current_exe()?;
    let dir = dirs_applications()?;
    std::fs::create_dir_all(&dir)?;
    if manifest.main_activity().is_none() {
        let old = dir.join(format!("aro-{}.desktop", manifest.package));
        if old.is_file() {
            let text = std::fs::read_to_string(&old)?;
            if !text.lines().any(|line| line == "NoDisplay=true") {
                std::fs::write(&old, format!("{text}\nNoDisplay=true\n"))?;
            }
        }
        println!("Installed {} (no launcher activity)", manifest.package);
        return Ok(());
    }

    // --- Extract icon ---
    let icon_name = format!("aro-{}", manifest.package);
    let icon_ref: String = if let Some((bytes, ext)) = aro_apk::extract_icon(&apk) {
        let icon_dir = dirs_icons()?;
        std::fs::create_dir_all(&icon_dir)?;
        let icon_path = icon_dir.join(format!("{icon_name}.{ext}"));
        std::fs::write(&icon_path, &bytes)?;
        log::info!("icon: wrote {}", icon_path.display());
        icon_path.to_string_lossy().into_owned()
    } else {
        // Fallback: generic Android icon name; hope the theme has one.
        "android".to_string()
    };

    // --- Visible launcher entry ---
    let human_name = match manifest.package.as_str() {
        "com.android.calendar" => "Calendar".to_string(),
        "com.android.deskclock" => "Clock".to_string(),
        "com.android.gallery3d" => "Gallery".to_string(),
        "com.android.music" => "Music".to_string(),
        "com.android.camera2" => "Camera".to_string(),
        "com.android.messaging" => "Messaging".to_string(),
        "com.android.contacts" => "Contacts".to_string(),
        "com.android.inputmethod.latin" => "Android Keyboard".to_string(),
        "org.chromium.webview_shell" => "Android Browser".to_string(),
        "net.sourceforge.opencamera" => "Open Camera".to_string(),
        _ => {
            let part = manifest.package.rsplit('.').next().unwrap_or(&manifest.package);
            let mut c = part.chars();
            match c.next() {
                Some(first) => first.to_uppercase().to_string() + c.as_str(),
                None => part.to_string(),
            }
        }
    };

    let launcher_entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Android app via ARO\n\
         Exec={arod} app {apk}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupWMClass={pkg}\n\
         X-ARO-Apk={apk_path}\n",
        name = human_name,
        apk_path = apk.to_string_lossy().replace('\\', "\\\\").replace('\n', "\\n"),
        arod = desktop_arg(&arod.to_string_lossy()),
        apk = desktop_arg(&apk.to_string_lossy()),
        icon = icon_ref,
        pkg = manifest.package,
    );
    let launcher_file = dir.join(format!("aro-{}.desktop", manifest.package));
    std::fs::write(&launcher_file, &launcher_entry)?;
    log::info!("wrote {}", launcher_file.display());
    println!("Installed launcher entry: {}", launcher_file.display());

    // --- Scheme handlers (deep links) ---
    // Only register custom, non-standard schemes. Standard system schemes
    // (http, https, file, content, mailto, etc.) belong to host applications
    // and must NEVER be hijacked via xdg-mime.
    let mut schemes: Vec<String> = Vec::new();
    for a in &manifest.activities {
        for f in &a.filters {
            if f.actions.iter().any(|x| x == "android.intent.action.VIEW") {
                for s in &f.schemes {
                    let s_clean = s.trim().to_ascii_lowercase();
                    if s_clean.is_empty() {
                        continue;
                    }
                    if matches!(s_clean.as_str(), "http" | "https" | "file" | "content" | "about" | "data" | "javascript" | "mailto" | "tel" | "geo") {
                        continue;
                    }
                    if !schemes.contains(&s_clean) {
                        schemes.push(s_clean);
                    }
                }
            }
        }
    }
    if !schemes.is_empty() {
        let mimetypes: String = schemes.iter().map(|s| format!("x-scheme-handler/{s};")).collect();
        let handler_entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=ARO: {pkg} (scheme handler)\n\
             Comment=Open {pkg} deep links via ARO\n\
             Exec={arod} app {apk} --url %u\n\
             Terminal=false\n\
             NoDisplay=true\n\
             MimeType={mimetypes}\n",
            pkg = manifest.package,
            arod = desktop_arg(&arod.to_string_lossy()),
            apk = desktop_arg(&apk.to_string_lossy()),
        );
        let handler_file = dir.join(format!("aro-{}-handler.desktop", manifest.package));
        std::fs::write(&handler_file, &handler_entry)?;
        log::info!("wrote {}", handler_file.display());
        let _ = std::process::Command::new("update-desktop-database").arg(&dir).status();
        let handler_name = handler_file.file_name().unwrap().to_string_lossy().into_owned();
        for s in &schemes {
            let st = std::process::Command::new("xdg-mime").args(["default", &handler_name, &format!("x-scheme-handler/{s}")]).status();
            match st {
                Ok(s2) if s2.success() => log::info!("registered scheme {s}:// -> {handler_name}"),
                _ => log::warn!("could not set default handler for {s}:// (xdg-mime)"),
            }
        }
        println!("Registered schemes: {}", schemes.iter().map(|s| format!("{s}://")).collect::<Vec<_>>().join(" "));
    }

    let _ = std::process::Command::new("update-desktop-database").arg(&dir).status();
    Ok(())
}

fn dirs_applications() -> Result<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).context("no HOME/XDG_DATA_HOME")?;
    Ok(base.join("applications"))
}

fn dirs_icons() -> Result<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))).context("no HOME/XDG_DATA_HOME")?;
    Ok(base.join("icons/hicolor/128x128/apps"))
}

fn open_uri(uri_or_path: &str) -> Result<()> {
    let uri = if url::Url::parse(uri_or_path).is_ok() {
        uri_or_path.to_string()
    } else {
        let abs = std::path::Path::new(uri_or_path).canonicalize().context("resolving path")?;
        url::Url::from_file_path(abs).map_err(|_| anyhow::anyhow!("invalid file path"))?.to_string()
    };

    let layout = Layout::default();
    let mut candidate_apks = Vec::new();
    let product_apps = layout.system.join("system/product/app");
    if let Ok(entries) = std::fs::read_dir(&product_apps) {
        for entry in entries.flatten() {
            if let Ok(subentries) = std::fs::read_dir(entry.path()) {
                for sub in subentries.flatten() {
                    if sub.path().extension().is_some_and(|e| e == "apk") {
                        candidate_apks.push(sub.path());
                    }
                }
            }
        }
    }
    let tmp_dir = layout.data.join("local/tmp");
    if let Ok(entries) = std::fs::read_dir(&tmp_dir) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "apk") {
                candidate_apks.push(entry.path());
            }
        }
    }

    let installed = layout.system.parent().unwrap().join("apps");
    if let Ok(entries) = std::fs::read_dir(installed) {
        for entry in entries.flatten() { let apk = entry.path().join("base.apk"); if apk.is_file() { candidate_apks.push(apk); } }
    }
    candidate_apks.sort();
    candidate_apks.dedup();
    for apk in &candidate_apks {
        if let Ok(manifest) = aro_apk::inspect(apk) {
            let spec = services::registry::AppSpec::from_manifest(&manifest, "/dummy".into(), 10001);
            if let Some(act) = services::activity::ActivityService::resolve(&spec, "android.intent.action.VIEW", Some(&uri)) {
                log::info!("open: {} handles {uri} via {act}", manifest.package);
                let arod = std::env::current_exe()?;
                let status = std::process::Command::new(arod)
                    .arg("app")
                    .arg(apk)
                    .arg("--url")
                    .arg(&uri)
                    .status()?;
                std::process::exit(status.code().unwrap_or(0));
            }
        }
    }

    bail!("no installed ARO app handles {uri}");
}

fn desktop_arg(value: &str) -> String {
    let exec = value.replace('%', "%%").replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$");
    format!("\"{}\"", exec.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r"))
}

fn android_file_uri(uri: &str, sdcard: Option<&std::path::Path>) -> String {
    let mapped = (|| {
        let host = url::Url::parse(uri).ok()?.to_file_path().ok()?;
        let relative = host.strip_prefix(sdcard?).ok()?;
        url::Url::from_file_path(std::path::Path::new("/sdcard").join(relative)).ok()
    })();
    mapped.map(|u| u.to_string()).unwrap_or_else(|| uri.to_string())
}

fn install_apk(apk: &std::path::Path) -> Result<()> {
    let source = apk.canonicalize()?;
    let manifest = aro_apk::inspect(&source)?;
    anyhow::ensure!(manifest.package.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')), "invalid package name");
    let dir = Layout::default().system.parent().unwrap().join("apps").join(&manifest.package);
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join("base.apk");
    if source != dest {
        let temp = dir.join("base.apk.new");
        std::fs::copy(&source, &temp)?;
        std::fs::rename(temp, &dest)?;
    }
    write_desktop_entry(&dest)
}

#[cfg(test)]
mod desktop_tests {
    use super::*;
    #[test]
    fn file_mapping_obeys_directory_boundaries_and_uri_encoding() {
        let home = Some(std::path::Path::new("/home/test"));
        assert_eq!(android_file_uri("file:///home/test/a%20b.ogg", home), "file:///sdcard/a%20b.ogg");
        assert_eq!(android_file_uri("file:///home/test-other/a.ogg", home), "file:///home/test-other/a.ogg");
        assert_eq!(android_file_uri("https://example.com/", home), "https://example.com/");
    }
    #[test]
    fn desktop_exec_escapes_field_codes_and_metacharacters() {
        assert_eq!(desktop_arg("/tmp/a b%f"), "\"/tmp/a b%%f\"");
        assert_eq!(desktop_arg("$`\""), "\"\\\\$\\\\`\\\\\"\"");
    }
}
