//! The Android process namespace: an unprivileged user namespace with its own
//! mount, PID and IPC namespaces, a root assembled from the unpacked image, and
//! a private binderfs instance.
use crate::layout::Layout;
use anyhow::{bail, Context, Result};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{fork, ForkResult, Pid};
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

pub struct Spec<'a> {
    pub layout: &'a Layout,
    /// Host files to bind at /data/local/tmp/<name>.
    pub extra_files: Vec<(PathBuf, String)>,
    pub env: Vec<(String, String)>,
    pub argv: Vec<String>,
    /// When set, we already run inside an arod session (user + IPC namespace,
    /// binderfs mounted, log socket owned by arod): only mount and PID
    /// namespaces are created here.
    pub session: Option<Session>,
}

/// Facts an arod session hands to app launchers through the environment.
#[derive(Clone, Debug)]
pub struct Session {
    /// Directory where the session's binderfs is mounted (arod's mount namespace).
    pub binderfs: PathBuf,
    /// Directory holding `logdw` (and later other sockets) to appear as /dev/socket.
    pub sockets: PathBuf,
}

impl Session {
    pub const ENV_BINDERFS: &'static str = "ARO_SESSION_BINDERFS";
    pub const ENV_SOCKETS: &'static str = "ARO_SESSION_SOCKETS";

    pub fn from_env() -> Option<Session> {
        let binderfs = std::env::var_os(Self::ENV_BINDERFS)?;
        let sockets = std::env::var_os(Self::ENV_SOCKETS)?;
        Some(Session { binderfs: binderfs.into(), sockets: sockets.into() })
    }
}

fn write_id_maps(uid: u32, gid: u32) -> Result<()> {
    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1\n")).context("uid_map")?;
    std::fs::write("/proc/self/setgroups", "deny").context("setgroups")?;
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1\n")).context("gid_map")?;
    Ok(())
}

fn bind(src: &Path, dst: &Path, ro: bool) -> Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dst)?;
    } else {
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        if !dst.exists() {
            std::fs::File::create(dst)?;
        }
    }
    mount(Some(src), dst, None::<&str>, MsFlags::MS_BIND | MsFlags::MS_REC, None::<&str>).with_context(|| format!("bind {} -> {}", src.display(), dst.display()))?;
    if ro {
        mount(None::<&str>, dst, None::<&str>, MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC, None::<&str>).ok();
    }
    Ok(())
}

fn tmpfs(dst: &Path, mode: &str) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    mount(Some("tmpfs"), dst, Some("tmpfs"), MsFlags::empty(), Some(format!("mode={mode}").as_str())).with_context(|| format!("tmpfs {}", dst.display()))
}

/// Mount a private binderfs at `bfs` and create the binder devices.
/// One instance exists per IPC namespace; mounting it again in the same IPC
/// namespace yields the same instance.
pub fn mount_binderfs(bfs: &Path) -> Result<()> {
    std::fs::create_dir_all(bfs)?;
    mount(Some("binder"), bfs, Some("binder"), MsFlags::empty(), None::<&str>).context("mount binderfs (kernel needs CONFIG_ANDROID_BINDERFS)")?;
    let ctl = std::fs::File::open(bfs.join("binder-control"))?;
    #[repr(C)]
    struct BinderfsDevice {
        name: [u8; 256],
        major: u32,
        minor: u32,
    }
    for name in ["binder", "hwbinder", "vndbinder"] {
        let mut d = BinderfsDevice { name: [0; 256], major: 0, minor: 0 };
        d.name[..name.len()].copy_from_slice(name.as_bytes());
        // BINDER_CTL_ADD = _IOWR('b', 1, struct binderfs_device)
        let req = (3u64 << 30) | ((std::mem::size_of::<BinderfsDevice>() as u64) << 16) | (98 << 8) | 1;
        let rc = unsafe { libc::ioctl(ctl.as_raw_fd(), req as _, &mut d as *mut _) };
        if rc != 0 {
            bail!("BINDER_CTL_ADD {name}: {}", std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// /dev/binder, /dev/hwbinder, /dev/vndbinder -> /dev/binderfs/<name>.
fn binder_symlinks(dev: &Path) -> Result<()> {
    for name in ["binder", "hwbinder", "vndbinder"] {
        std::os::unix::fs::symlink(format!("binderfs/{name}"), dev.join(name))?;
    }
    Ok(())
}

/// Assemble the root filesystem at `root` from the layout. Runs inside the new namespaces.
fn assemble_root(spec: &Spec, root: &Path) -> Result<()> {
    let l = spec.layout;
    // Make every mount private so nothing leaks to the host.
    mount(None::<&str>, "/", None::<&str>, MsFlags::MS_REC | MsFlags::MS_PRIVATE, None::<&str>).context("make / private")?;
    tmpfs(root, "755")?;

    bind(&l.system.join("system"), &root.join("system"), true)?;
    bind(&l.system.join("apex"), &root.join("apex"), true)?;
    bind(&l.data, &root.join("data"), false)?;
    bind(&l.state.join("linkerconfig"), &root.join("linkerconfig"), false)?;
    bind(&l.state.join("metadata"), &root.join("metadata"), true)?;
    for d in ["vendor", "odm", "mnt", "storage", "sdcard", "config"] {
        std::fs::create_dir_all(root.join(d))?;
    }
    // GSI keeps system_ext and product inside /system; expose them at / as the image does.
    for d in ["system_ext", "product"] {
        if l.system.join("system").join(d).is_dir() {
            std::os::unix::fs::symlink(format!("/system/{d}"), root.join(d))?;
        } else {
            std::fs::create_dir_all(root.join(d))?;
        }
    }
    for (link, target) in [("bin", "/system/bin"), ("etc", "/system/etc")] {
        std::os::unix::fs::symlink(target, root.join(link))?;
    }

    // /dev: a fresh tmpfs with the few host nodes Android needs, plus ours.
    let dev = root.join("dev");
    tmpfs(&dev, "755")?;
    for n in ["null", "zero", "full", "random", "urandom", "tty"] {
        bind(&Path::new("/dev").join(n), &dev.join(n), false)?;
    }
    std::fs::create_dir_all(dev.join("pts"))?;
    mount(Some("devpts"), &dev.join("pts"), Some("devpts"), MsFlags::empty(), Some("newinstance,ptmxmode=0666,mode=0620")).ok();
    std::fs::create_dir_all(dev.join("shm"))?;
    tmpfs(&dev.join("shm"), "1777")?;
    bind(&l.state.join("props"), &dev.join("__properties__"), true)?;
    match &spec.session {
        Some(s) => {
            bind(&s.sockets, &dev.join("socket"), false)?;
            bind(&s.binderfs, &dev.join("binderfs"), false)?;
        }
        None => {
            bind(&l.runtime.join("socket"), &dev.join("socket"), false)?;
            mount_binderfs(&dev.join("binderfs"))?;
        }
    }
    binder_symlinks(&dev)?;
    if Path::new("/dev/dri").exists() {
        bind(Path::new("/dev/dri"), &dev.join("dri"), false)?;
    }

    tmpfs(&root.join("tmp"), "1777")?;
    std::fs::create_dir_all(root.join("proc"))?;
    mount(Some("proc"), &root.join("proc"), Some("proc"), MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC, None::<&str>).context("mount proc")?;
    std::fs::create_dir_all(root.join("sys"))?;
    bind(Path::new("/sys"), &root.join("sys"), true).ok();

    for (src, name) in &spec.extra_files {
        bind(src, &root.join("data/local/tmp").join(name), true)?;
    }
    bind(&l.bootstrap_jar, &root.join("data/local/tmp/aro-bootstrap.jar"), true)?;
    Ok(())
}

fn pivot(root: &Path) -> Result<()> {
    let old = root.join(".host");
    std::fs::create_dir_all(&old)?;
    nix::unistd::pivot_root(root, &old).context("pivot_root")?;
    nix::unistd::chdir("/")?;
    umount2("/.host", MntFlags::MNT_DETACH).context("detach old root")?;
    std::fs::remove_dir("/.host").ok();
    Ok(())
}

/// Run a command inside the (already pivoted) namespace and capture stdout.
fn run_inside(argv: &[&str], env: &[(String, String)]) -> Result<String> {
    let out = std::process::Command::new(argv[0]).args(&argv[1..]).env_clear().envs(env.iter().map(|(k, v)| (k, v))).output().with_context(|| format!("running {}", argv[0]))?;
    if !out.status.success() {
        bail!("{} failed: {}", argv[0], out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Generate ld.config.txt and the classpath exports inside the namespace, once.
fn derive_inside(env: &mut Vec<(String, String)>) -> Result<()> {
    if !Path::new("/linkerconfig/ld.config.txt").exists() {
        run_inside(&["/apex/com.android.runtime/bin/linkerconfig", "--target", "/linkerconfig"], env)?;
    }
    let cp = Path::new("/data/local/tmp/classpath.env");
    if !cp.exists() {
        run_inside(&["/apex/com.android.sdkext/bin/derive_classpath", "/data/local/tmp/classpath.env"], env)?;
    }
    for line in std::fs::read_to_string(cp)?.lines() {
        let mut it = line.splitn(3, ' ');
        if let (Some("export"), Some(k), Some(v)) = (it.next(), it.next(), it.next()) {
            env.push((k.to_string(), v.to_string()));
        }
    }
    Ok(())
}

fn child(spec: &Spec, root: &Path, notif_sock: std::os::fd::RawFd) -> Result<()> {
    assemble_root(spec, root)?;
    pivot(root)?;
    let mut env: Vec<(String, String)> = [
        ("PATH", "/system/bin:/apex/com.android.art/bin"),
        ("ANDROID_ROOT", "/system"),
        ("ANDROID_DATA", "/data"),
        ("ANDROID_STORAGE", "/storage"),
        ("ANDROID_ART_ROOT", "/apex/com.android.art"),
        ("ANDROID_I18N_ROOT", "/apex/com.android.i18n"),
        ("ANDROID_TZDATA_ROOT", "/apex/com.android.tzdata"),
        ("EXTERNAL_STORAGE", "/sdcard"),
        ("TMPDIR", "/tmp"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    derive_inside(&mut env)?;
    env.extend(spec.env.iter().cloned());

    let prog = CString::new(spec.argv[0].as_str())?;
    let args: Vec<CString> = spec.argv.iter().map(|a| CString::new(a.as_str())).collect::<Result<_, _>>()?;
    let envp: Vec<CString> = env.iter().map(|(k, v)| CString::new(format!("{k}={v}"))).collect::<Result<_, _>>()?;
    // Last thing before exec: the syscall supervisor filter, listener handed to the parent.
    let listener = crate::seccomp::install()?;
    if std::env::var_os("ARO_DEBUG").is_some() { eprintln!("aro-exec[child]: seccomp listener fd {}", std::os::fd::AsRawFd::as_raw_fd(&listener)); }
    crate::seccomp::send_fd(notif_sock, std::os::fd::AsRawFd::as_raw_fd(&listener))?;
    if std::env::var_os("ARO_DEBUG").is_some() { eprintln!("aro-exec[child]: listener sent, exec {}", spec.argv[0]); }
    drop(listener);
    nix::unistd::close(notif_sock).ok();
    nix::unistd::execve(&prog, &args, &envp).context("execve")?;
    Ok(())
}

/// Enter the namespaces and run `spec.argv` as PID 1 of a new PID namespace.
/// Returns the child's exit status.
pub fn run(spec: &Spec, log: Option<&std::os::unix::net::UnixDatagram>) -> Result<i32> {
    let root = spec.layout.runtime.join(format!("root-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(spec.layout.runtime.join("socket"))?;

    if spec.session.is_some() {
        // arod already put us in the session's user and IPC namespaces.
        unshare(CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS).context("unshare")?;
    } else {
        // Read our ids before unshare: inside the new namespace they read as the overflow id.
        let (uid, gid) = (nix::unistd::getuid().as_raw(), nix::unistd::getgid().as_raw());
        unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWIPC | CloneFlags::CLONE_NEWUTS).context("unshare (kernel.unprivileged_userns_clone must be 1)")?;
        write_id_maps(uid, gid)?;
    }
    nix::unistd::sethostname("aro").ok();

    // Socket pair over which the child hands back its seccomp listener.
    let (parent_sock, child_sock) = nix::sys::socket::socketpair(nix::sys::socket::AddressFamily::Unix, nix::sys::socket::SockType::Stream, None, nix::sys::socket::SockFlag::SOCK_CLOEXEC).context("socketpair")?;
    match unsafe { fork() }.context("fork")? {
        ForkResult::Child => {
            drop(parent_sock);
            let code = match child(spec, &root, std::os::fd::IntoRawFd::into_raw_fd(child_sock)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("aro-exec: {e:#}");
                    1
                }
            };
            std::process::exit(code);
        }
        ForkResult::Parent { child } => {
            drop(child_sock);
            if std::env::var_os("ARO_DEBUG").is_some() { eprintln!("aro-exec[parent]: waiting for listener"); }
            let listener = match crate::seccomp::recv_fd(std::os::fd::AsRawFd::as_raw_fd(&parent_sock)) {
                Ok(fd) => { if std::env::var_os("ARO_DEBUG").is_some() { eprintln!("aro-exec[parent]: got listener"); } Some(fd) }
                Err(e) => {
                    eprintln!("aro-exec: no seccomp listener from child ({e:#}); syscall supervision off");
                    None
                }
            };
            wait_child(child, log, listener)
        }
    }
}

/// Wait for the child, pumping Android log datagrams to stderr when we own the
/// socket and answering supervised syscalls on the seccomp listener.
fn wait_child(child: Pid, log: Option<&std::os::unix::net::UnixDatagram>, mut listener: Option<std::os::fd::OwnedFd>) -> Result<i32> {
    use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
    use nix::sys::wait::WaitPidFlag;
    use std::os::fd::{AsFd, AsRawFd};
    let mut buf = vec![0u8; 65536];
    loop {
        {
            let mut fds: Vec<PollFd> = Vec::new();
            if let Some(log) = log {
                fds.push(PollFd::new(log.as_fd(), PollFlags::POLLIN));
            }
            if let Some(l) = &listener {
                fds.push(PollFd::new(l.as_fd(), PollFlags::POLLIN));
            }
            let _ = poll(&mut fds, PollTimeout::from(100u16));
        }
        if let Some(log) = log {
            crate::logd::drain(log, &mut buf);
        }
        let mut drop_listener = false;
        if let Some(l) = &listener {
            // Drain every pending notification without blocking. When the last
            // filtered process is gone the listener reports hang-up: stop then.
            loop {
                let mut fds = [PollFd::new(l.as_fd(), PollFlags::POLLIN)];
                match poll(&mut fds, PollTimeout::ZERO) {
                    Ok(n) if n > 0 => {
                        let ev = fds[0].revents().unwrap_or(PollFlags::empty());
                        if ev.intersects(PollFlags::POLLHUP | PollFlags::POLLERR) {
                            drop_listener = true;
                            break;
                        }
                        match crate::seccomp::serve_one(l.as_raw_fd()) {
                            Ok(true) => continue,
                            Ok(false) => break,
                            Err(e) => {
                                eprintln!("aro-exec: seccomp: {e:#}");
                                drop_listener = true;
                                break;
                            }
                        }
                    }
                    _ => break,
                }
            }
        }
        if drop_listener {
            listener = None;
        }
        let status = waitpid(child, Some(WaitPidFlag::WNOHANG))?;
        match status {
            WaitStatus::Exited(_, code) => {
                if let Some(log) = log {
                    crate::logd::drain(log, &mut buf);
                }
                return Ok(code);
            }
            WaitStatus::Signaled(_, sig, _) => {
                if let Some(log) = log {
                    crate::logd::drain(log, &mut buf);
                }
                return Ok(128 + sig as i32);
            }
            _ => continue,
        }
    }
}
