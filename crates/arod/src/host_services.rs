//! Supervised native Binder workers. Each worker kind below owns its process;
//! MediaProvider's cursor and file-picker control share its session state.
use crate::services::{self, Service};
use anyhow::{Context, Result};
use rsbinder::{Parcel, SIBinder};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicI32, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{Duration, Instant};

pub const DOCUMENTS: &str = "aro.documents";
pub const MEDIA_PROVIDER: &str = "aro.media.provider";
pub const CALENDAR_PROVIDER: &str = "aro.calendar.provider";
pub const SETTINGS_PROVIDER: &str = "aro.settings.provider";
pub const LOCATION_AUTHORITY: &str = "aro.location.authority";

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum Kind {
    Connectivity,
    Audio,
    AudioFlinger,
    AudioPolicy,
    MediaPlayer,
    Storage,
    MediaProvider,
    CalendarProvider,
    SettingsProvider,
    Clipboard,
    Location,
}
impl Kind {
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Self::Connectivity => &["connectivity"],
            Self::Audio => &["audio"],
            Self::AudioFlinger => &["media.audio_flinger"],
            Self::AudioPolicy => &["media.audio_policy"],
            Self::MediaPlayer => &["media.player"],
            Self::Storage => &["mount"],
            Self::MediaProvider => &[MEDIA_PROVIDER, DOCUMENTS],
            Self::CalendarProvider => &[CALENDAR_PROVIDER],
            Self::SettingsProvider => &[SETTINGS_PROVIDER],
            Self::Clipboard => &["clipboard"],
            Self::Location => &["location"],
        }
    }
    fn argument(self) -> &'static str {
        match self {
            Self::Connectivity => "connectivity",
            Self::Audio => "audio",
            Self::AudioFlinger => "audio-flinger",
            Self::AudioPolicy => "audio-policy",
            Self::MediaPlayer => "media-player",
            Self::Storage => "storage",
            Self::MediaProvider => "media-provider",
            Self::Clipboard => "clipboard",
            Self::Location => "location",
            Self::CalendarProvider => "calendar-provider",
            Self::SettingsProvider => "settings-provider",
        }
    }
}

struct Worker {
    child: Child,
    kind: Kind,
    setup: Option<Arc<crate::location_setup::HostSetup>>,
}
pub struct Workers {
    children: Arc<Mutex<Vec<Worker>>>,
    hub: Arc<crate::hub::Hub>,
    binder: std::path::PathBuf,
    host_uid: u32,
}
impl Workers {
    pub fn new(hub: Arc<crate::hub::Hub>, binder: std::path::PathBuf, host_uid: u32) -> Self {
        let children = Arc::new(Mutex::new(Vec::<Worker>::new()));
        let weak = Arc::downgrade(&children);
        let monitor_hub = hub.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(100));
            let Some(children) = weak.upgrade() else {
                break;
            };
            let mut children = children.lock().unwrap();
            children.retain_mut(|worker| match worker.child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!(
                        "host-service {:?} pid={} exited: {status}",
                        worker.kind,
                        worker.child.id()
                    );
                    monitor_hub.remove_process(worker.child.id() as i32);
                    if let Some(setup) = &worker.setup {
                        setup.shutdown();
                    }
                    false
                }
                Ok(None) => true,
                Err(e) => {
                    log::error!("host-service wait: {e}");
                    true
                }
            });
        });
        Self {
            children,
            hub,
            binder,
            host_uid,
        }
    }

    pub fn spawn(
        &self,
        kind: Kind,
        setup: Option<Arc<crate::location_setup::HostSetup>>,
    ) -> Result<i32> {
        let mut command = Command::new(std::env::current_exe()?);
        let parent_pid = unsafe { libc::getpid() };
        command
            .arg("host-service")
            .arg(kind.argument())
            .arg("--binder")
            .arg(&self.binder)
            .arg("--host-uid")
            .arg(self.host_uid.to_string())
            .arg("--supervisor-pid")
            .arg(parent_pid.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        let setup_fd = setup.as_ref().map(|s| s.duplicate_socket()).transpose()?;
        let raw = setup_fd.as_ref().map(AsRawFd::as_raw_fd);
        if let Some(fd) = raw {
            command.env("ARO_LOCATION_SETUP_FD", fd.to_string());
        }
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent_pid {
                    return Err(std::io::Error::other("supervisor exited"));
                }
                if let Some(fd) = raw {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn().context("spawn Binder worker")?;
        let pid = child.id() as i32;
        // The worker waits for this byte before registering, so its names cannot
        // be claimed by an app in the spawn-to-registration interval.
        self.hub.reserve(kind.names(), pid);
        let start = (|| -> Result<()> {
            child
                .stdin
                .take()
                .context("worker startup pipe")?
                .write_all(b"G")?;
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if kind
                    .names()
                    .iter()
                    .all(|name| self.hub.owned_service(name, pid).is_some())
                {
                    return Ok(());
                }
                if let Some(status) = child.try_wait()? {
                    anyhow::bail!("{kind:?} exited before registration: {status}");
                }
                anyhow::ensure!(Instant::now() < deadline, "{kind:?} registration timed out");
                std::thread::sleep(Duration::from_millis(20));
            }
        })();
        if let Err(e) = start {
            let _ = child.kill();
            let _ = child.wait();
            self.hub.remove_process(pid);
            return Err(e);
        }
        log::info!(
            "host-service {kind:?} ready pid={pid} names={:?}",
            kind.names()
        );
        self.children
            .lock()
            .unwrap()
            .push(Worker { child, kind, setup });
        Ok(pid)
    }

    pub fn service(&self, name: &str) -> Result<SIBinder> {
        for worker in self.children.lock().unwrap().iter() {
            if let Some(binder) = self.hub.owned_service(name, worker.child.id() as i32) {
                return Ok(binder);
            }
        }
        anyhow::bail!("worker did not publish {name}")
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        for worker in self.children.lock().unwrap().iter_mut() {
            if let Some(setup) = &worker.setup {
                setup.shutdown();
            }
            let _ = worker.child.kill();
            let _ = worker.child.wait();
            self.hub.remove_process(worker.child.id() as i32);
        }
    }
}

fn publish<S: Service>(name: &str, service: S) -> Result<()> {
    rsbinder::hub::add_service(name, services::binder_of(service))
        .map_err(|e| anyhow::anyhow!("register {name}: {e:?}"))
}

pub fn serve(
    kind: Kind,
    binder: &std::path::Path,
    host_uid: u32,
    supervisor_pid: i32,
) -> Result<()> {
    let mut go = [0];
    std::io::stdin().read_exact(&mut go)?;
    anyhow::ensure!(
        go == *b"G" && unsafe { libc::getppid() } == supervisor_pid,
        "invalid worker startup"
    );
    rsbinder::ProcessState::init(binder.to_str().context("binder path")?, 0)
        .map_err(|e| anyhow::anyhow!("worker Binder: {e:?}"))?;
    rsbinder::ProcessState::start_thread_pool();
    match kind {
        Kind::Connectivity => {
            let net = crate::hostnet::probe(host_uid);
            publish(
                "connectivity",
                services::network::NetworkService {
                    net: crate::hostnet::monitor(host_uid, net),
                },
            )?;
        }
        Kind::Audio => publish("audio", services::audio::AudioService)?,
        Kind::AudioFlinger => publish(
            "media.audio_flinger",
            services::audio_flinger::AudioFlingerService,
        )?,
        Kind::AudioPolicy => publish(
            "media.audio_policy",
            services::audio_policy::AudioPolicyService,
        )?,
        Kind::MediaPlayer => publish("media.player", services::media_player::MediaPlayerService)?,
        Kind::Storage => publish("mount", services::storage::StorageService)?,
        Kind::Clipboard => publish(
            "clipboard",
            services::clipboard::ClipboardService::default(),
        )?,
        Kind::MediaProvider => {
            let layout = aro_exec::layout::Layout::default();
            let service = Arc::new(services::media::MediaService::load(
                &layout.system,
                &layout.data,
            ));
            publish(
                MEDIA_PROVIDER,
                services::media::MediaProvider {
                    service: service.clone(),
                    bulk_cursor: services::binder_of(services::cursor::BulkCursorService),
                },
            )?;
            publish(
                DOCUMENTS,
                Documents {
                    service,
                    host_uid,
                    supervisor_pid,
                },
            )?;
        }
        Kind::CalendarProvider => {
            let layout = aro_exec::layout::Layout::default();
            publish(
                CALENDAR_PROVIDER,
                services::calendar::CalendarProvider {
                    service: Arc::new(services::calendar::CalendarService::load(&layout.data)),
                    bulk_cursor: services::binder_of(services::cursor::BulkCursorService),
                },
            )?;
        }
        Kind::SettingsProvider => {
            let layout = aro_exec::layout::Layout::default();
            publish(
                SETTINGS_PROVIDER,
                services::settings::SettingsProvider {
                    service: Arc::new(services::settings::SettingsService::load(&layout.data)),
                },
            )?;
        }
        Kind::Location => {
            let raw: i32 = std::env::var("ARO_LOCATION_SETUP_FD")
                .context("missing host setup socket")?
                .parse()?;
            anyhow::ensure!(raw > 2, "invalid setup fd");
            let socket = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw) };
            unsafe {
                libc::fcntl(socket.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            }
            std::env::remove_var("ARO_LOCATION_SETUP_FD");
            let authority = rsbinder::hub::check_service(LOCATION_AUTHORITY)
                .context("location caller authority missing")?;
            publish(
                "location",
                services::location::LocationService {
                    host_uid,
                    setup: Some(Arc::new(crate::location_setup::HostSetup::from_socket(
                        socket,
                    )?)),
                    authority,
                },
            )?;
        }
    }
    rsbinder::ProcessState::join_thread_pool()
        .map_err(|e| anyhow::anyhow!("worker Binder loop: {e:?}"))
}

struct Documents {
    service: Arc<services::media::MediaService>,
    host_uid: u32,
    supervisor_pid: i32,
}
impl Service for Documents {
    const DESCRIPTOR: &'static str = "org.aro.IDocuments";
    const TABLE: &'static [(u32, &'static str)] = &[(1, "choose")];
    fn handle(
        &self,
        name: &str,
        _: u32,
        data: &mut Parcel,
        reply: &mut Parcel,
    ) -> rsbinder::Result<bool> {
        if name != "choose" {
            return Ok(false);
        }
        if rsbinder::thread_state::get_calling_pid() != self.supervisor_pid {
            return Err(rsbinder::StatusCode::PermissionDenied);
        }
        let package: Option<String> = data.read()?;
        let mime: Option<String> = data.read()?;
        let uri = crate::portal::open_file(
            self.host_uid,
            package.as_deref().unwrap_or("Android app"),
            mime.as_deref(),
        )
        .and_then(|path| {
            path.map(|p| self.service.select_document(p).map_err(Into::into))
                .transpose()
        })
        .unwrap_or_else(|e| {
            log::warn!("documents: selection failed: {e}");
            None
        });
        reply.write(&uri)?;
        Ok(true)
    }
}
pub fn choose_document(
    control: &SIBinder,
    package: &str,
    mime: Option<&str>,
) -> Result<Option<String>> {
    let proxy = control.as_proxy().context("documents is not remote")?;
    let mut data = proxy.prepare_transact(true)?;
    data.write(&Some(package))?;
    data.write(&mime)?;
    let mut reply = proxy
        .submit_transact(1, &data, 0)?
        .context("empty document reply")?;
    Ok(reply.read()?)
}

pub struct LocationAuthority {
    pub activity: Weak<services::activity::ActivityService>,
    pub worker_pid: Arc<AtomicI32>,
}
impl Service for LocationAuthority {
    const DESCRIPTOR: &'static str = "org.aro.ILocationAuthority";
    const TABLE: &'static [(u32, &'static str)] = &[(1, "validate")];
    fn handle(
        &self,
        name: &str,
        _: u32,
        data: &mut Parcel,
        reply: &mut Parcel,
    ) -> rsbinder::Result<bool> {
        if name != "validate" {
            return Ok(false);
        }
        if rsbinder::thread_state::get_calling_pid() != self.worker_pid.load(Ordering::Acquire) {
            return Err(rsbinder::StatusCode::PermissionDenied);
        }
        let pid = data.read_i32()?;
        let package: Option<String> = data.read()?;
        let allowed = self.activity.upgrade().is_some_and(|activity| {
            pid > 0
                && pid == activity.service_processes.app_pid.load(Ordering::Acquire)
                && activity
                    .attached
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|(_, app)| package.as_deref() == Some(&app.package))
        });
        reply.write_i32(allowed as i32)?;
        Ok(true)
    }
}
pub fn validate_location_caller(
    authority: &SIBinder,
    pid: i32,
    package: Option<&str>,
) -> rsbinder::Result<bool> {
    let proxy = authority.as_proxy().ok_or(rsbinder::StatusCode::BadType)?;
    let mut data = proxy.prepare_transact(true)?;
    data.write_i32(pid)?;
    data.write(&package)?;
    let mut reply = proxy
        .submit_transact(1, &data, 0)?
        .ok_or(rsbinder::StatusCode::NotEnoughData)?;
    Ok(reply.read_i32()? != 0)
}
