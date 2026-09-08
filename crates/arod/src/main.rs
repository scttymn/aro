//! arod: the ARO session supervisor.
//!
//! Creates the session namespaces, mounts the private binderfs, becomes the
//! Binder context manager, hosts the native services, sinks Android logs,
//! and launches app processes through `aro-exec`.
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
mod compositor;
mod notify;
mod input_channel;
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
    /// Start a session, then run the M1 bootstrap against an APK inside it
    Run {
        apk: PathBuf,
        class: String,
        method: Option<String>,
    },
    /// Start a session and run a shell inside it
    Shell,
    /// Start a session and launch an APK as a real app process (ActivityThread.main)
    App {
        apk: PathBuf,
        /// Activity to launch (default: the manifest's LAUNCHER activity)
        #[arg(long)]
        activity: Option<String>,
    },
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).format_timestamp_millis().init();
    let cli = Cli::parse();
    let layout = Layout::default();
    if !layout.system.join("system/bin/app_process64").exists() {
        bail!("no unpacked system image at {} (run: aro-image unpack)", layout.system.display());
    }

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
    let activity = std::sync::Arc::new(services::activity::ActivityService { registry: registry.clone(), pending: std::sync::Mutex::new(None), attached: std::sync::Mutex::new(None), client_controller: std::sync::Mutex::new(None) });
    let controller = services::binder_of(services::activity_task::ActivityClientController);
    *activity.client_controller.lock().unwrap() = Some(controller.clone());
    services::publish(&hub_impl, "activity_task", services::activity_task::ActivityTaskService { client_controller: std::sync::Mutex::new(Some(controller)) });
    // Gralloc (allocator + mapper) — created early so the composer can resolve
    // posted buffers to their memfds for the Wayland presenter.
    let gralloc = std::sync::Arc::new(services::allocator::Gralloc::default());
    let input_hub = std::sync::Arc::new(input_channel::InputHub::default());
    // Composer + window manager.
    let sf = std::sync::Arc::new(services::surfaceflinger::SurfaceFlinger::new(hd.width as u32, hd.height as u32, hd.frame_interval_ns()));
    let composer_client = services::binder_of(services::surfaceflinger::ComposerClient { sf: sf.clone() });
    *sf.client.lock().unwrap() = Some(composer_client.clone());
    let display_token = services::token::new_token("display");
    services::publish(&hub_impl, "SurfaceFlingerAIDL", services::surfaceflinger::ComposerAidl { sf: sf.clone(), client: composer_client, display_token });
    let window_host = std::sync::Arc::new(services::window_session::WindowHost::new(sf.clone(), (hd.width, hd.height), hd.dpi(), hd.scale, input_hub.clone()));
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
    services::publish(&hub_impl, "audio", services::audio::AudioService);
    // Notifications go to whatever owns org.freedesktop.Notifications on the session bus.
    let notifier = match notify::Notifier::connect(session.host_uid) {
        Ok(n) => Some(std::sync::Arc::new(n)),
        Err(e) => { log::warn!("notify: no host notification service ({e}); notifications logged only"); None }
    };
    services::publish(&hub_impl, "notification", services::notification::NotificationService { notifier });
    // Network: mirror the host's connection (NetworkManager on the system bus).
    if let Err(e) = dnsproxy::serve(&session.sockets) { log::warn!("dnsproxyd: {e}"); }
    let hostnet = hostnet::probe(session.host_uid);
    services::publish(&hub_impl, "connectivity", services::network::NetworkService { net: hostnet.clone() });
    // The allocator is a VINTF-stable HAL binder; the mapper half is a bionic
    // library bound into the app at /vendor/lib64/hw/mapper.aro.so.
    hub_impl.register("android.hardware.graphics.allocator.IAllocator/default", services::vintf_binder_of(services::allocator::AllocatorService { gralloc: gralloc.clone() }));
    let vendor_dir = prepare_vendor_dir(&layout)?;
    services::publish(&hub_impl, "accessibility", services::accessibility::AccessibilityService);
    services::publish(&hub_impl, "user", services::user::UserService);
    services::publish(&hub_impl, "mount", services::storage::StorageService);
    services::publish(&hub_impl, "sensorservice", services::sensor::SensorService);
    services::publish(&hub_impl, "media.camera", services::camera::CameraService);
    services::publish(&hub_impl, "package", services::package::PackageService { registry: registry.clone() });
    services::publish(&hub_impl, "platform_compat", services::compat::PlatformCompatService::load(&layout.system, registry.clone()));
    services::publish(&hub_impl, "display", services::display::DisplayService { name: hd.name.clone(), width: hd.width, height: hd.height, dpi: hd.dpi(), callbacks: std::sync::Mutex::new(Vec::new()) });
    services::publish(&hub_impl, "activity", services::activity::ActivityRef(activity.clone()));

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
    cmd.env(Session::ENV_BINDERFS, &session.binderfs).env(Session::ENV_SOCKETS, &session.sockets);
    if let Some(v) = &vendor_dir {
        cmd.env("ARO_VENDOR_DIR", v);
    }
    // External storage maps to a host directory: ARO_SDCARD, else the user's home.
    let sdcard = std::env::var_os("ARO_SDCARD").or_else(|| std::env::var_os("HOME"));
    if let Some(sd) = sdcard {
        cmd.env("ARO_SDCARD", sd);
    }
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
        Cmd::App { apk, activity: main_activity } => {
            let apk = apk.canonicalize()?;
            let name = apk.file_name().unwrap().to_string_lossy().into_owned();
            let manifest = manifest.expect("manifest parsed above");
            let package = manifest.package.clone();
            log::info!("arod: {} v{} target sdk {} app {:?} launcher {:?}", package, manifest.version_code, manifest.target_sdk, manifest.app_class, manifest.main_activity().map(|a| &a.name));
            for d in ["data", "user_de/0"] {
                std::fs::create_dir_all(layout.data.join(d).join(&package))?;
            }
            let user0 = layout.data.join("user/0");
            if !user0.exists() {
                std::fs::create_dir_all(layout.data.join("user"))?;
                std::os::unix::fs::symlink("../data", &user0)?;
            }
            let mut spec = services::registry::AppSpec::from_manifest(&manifest, format!("/data/local/tmp/{name}"), 10001);
            if let Some(a) = main_activity {
                spec.main_activity = Some(a);
            }
            registry.apps.lock().unwrap().push(spec.clone());
            *activity.pending.lock().unwrap() = Some(spec);
            cmd.arg("run").arg("--app").arg(&apk);
        }
    }
    let status = cmd.status().with_context(|| format!("spawning {}", exe.display()))?;
    log::info!("arod: app exited with {status}");
    std::process::exit(status.code().unwrap_or(1));
}

/// Assemble the app's /vendor: `lib64/hw/mapper.aro.so` (built from crates/aro-mapper
/// for x86_64-linux-android). Looked up via ARO_MAPPER_SO, then next to the arod
/// binary as `mapper.aro.so`, then in the cargo target tree.
fn prepare_vendor_dir(layout: &aro_exec::layout::Layout) -> anyhow::Result<Option<std::path::PathBuf>> {
    let exe_dir = std::env::current_exe()?.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let candidates = [
        std::env::var_os("ARO_MAPPER_SO").map(std::path::PathBuf::from),
        Some(exe_dir.join("mapper.aro.so")),
        exe_dir.parent().map(|t| t.join("x86_64-linux-android").join("debug").join("libaro_mapper.so")),
        exe_dir.parent().map(|t| t.join("x86_64-linux-android").join("release").join("libaro_mapper.so")),
    ];
    let Some(src) = candidates.into_iter().flatten().find(|p| p.is_file()) else {
        log::warn!("gralloc: mapper.aro.so not found (build with `cargo build -p aro-mapper --target x86_64-linux-android`); apps cannot allocate buffers");
        return Ok(None);
    };
    let vendor = layout.runtime.join("vendor");
    let hw = vendor.join("lib64").join("hw");
    std::fs::create_dir_all(&hw)?;
    std::fs::copy(&src, hw.join("mapper.aro.so"))?;
    log::info!("gralloc: mapper {} -> {}", src.display(), hw.join("mapper.aro.so").display());
    Ok(Some(vendor))
}
