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

    // 3b. Services.
    let registry = std::sync::Arc::new(services::registry::Registry { apps: std::sync::Mutex::new(Vec::new()), display: (1280, 800, 160) });
    let activity = std::sync::Arc::new(services::activity::ActivityService { registry: registry.clone(), pending: std::sync::Mutex::new(None), attached: std::sync::Mutex::new(None), client_controller: std::sync::Mutex::new(None) });
    let controller = services::binder_of(services::activity_task::ActivityClientController);
    *activity.client_controller.lock().unwrap() = Some(controller.clone());
    services::publish(&hub_impl, "activity_task", services::activity_task::ActivityTaskService { client_controller: std::sync::Mutex::new(Some(controller)) });
    // Composer + window manager.
    let sf = std::sync::Arc::new(services::surfaceflinger::SurfaceFlinger::new(1280, 800));
    let composer_client = services::binder_of(services::surfaceflinger::ComposerClient { sf: sf.clone() });
    *sf.client.lock().unwrap() = Some(composer_client.clone());
    let display_token = services::token::new_token("display");
    services::publish(&hub_impl, "SurfaceFlingerAIDL", services::surfaceflinger::ComposerAidl { sf: sf.clone(), client: composer_client, display_token });
    services::publish(&hub_impl, "SurfaceFlinger", services::surfaceflinger::ComposerLegacy { sf: sf.clone(), transactions: std::sync::atomic::AtomicU32::new(0) });
    let window_session = services::binder_of(services::window_session::WindowSession { sf: sf.clone(), display: (1280, 800, 160), windows: std::sync::Mutex::new(Vec::new()) });
    services::publish(&hub_impl, "window", services::window::WindowService { session: window_session });
    services::publish(&hub_impl, "input_method", services::input_method::InputMethodService);
    services::publish(&hub_impl, "input", services::input::InputService);
    // Gralloc: the allocator is a VINTF-stable HAL binder; the mapper half is a
    // bionic library bound into the app at /vendor/lib64/hw/mapper.aro.so.
    let gralloc = std::sync::Arc::new(services::allocator::Gralloc::default());
    hub_impl.register("android.hardware.graphics.allocator.IAllocator/default", services::vintf_binder_of(services::allocator::AllocatorService { gralloc: gralloc.clone() }));
    let vendor_dir = prepare_vendor_dir(&layout)?;
    services::publish(&hub_impl, "accessibility", services::accessibility::AccessibilityService);
    services::publish(&hub_impl, "user", services::user::UserService);
    services::publish(&hub_impl, "sensorservice", services::sensor::SensorService);
    services::publish(&hub_impl, "media.camera", services::camera::CameraService);
    services::publish(&hub_impl, "package", services::package::PackageService { registry: registry.clone() });
    services::publish(&hub_impl, "platform_compat", services::compat::PlatformCompatService::load(&layout.system, registry.clone()));
    services::publish(&hub_impl, "display", services::display::DisplayService { width: 1280, height: 800, dpi: 160, callbacks: std::sync::Mutex::new(Vec::new()) });
    services::publish(&hub_impl, "activity", services::activity::ActivityRef(activity.clone()));

    // Properties are cheap to regenerate and ARO's extra ones evolve with the runtime.
    let n = prepare::write_properties(&layout.system, &layout.state)?;
    log::info!("arod: {n} system properties");

    // 4. Launch.
    let exe = std::env::current_exe()?.with_file_name("aro-exec");
    let mut cmd = std::process::Command::new(&exe);
    cmd.env(Session::ENV_BINDERFS, &session.binderfs).env(Session::ENV_SOCKETS, &session.sockets);
    if let Some(v) = &vendor_dir {
        cmd.env("ARO_VENDOR_DIR", v);
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
            let manifest = aro_apk::inspect(&apk)?;
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
