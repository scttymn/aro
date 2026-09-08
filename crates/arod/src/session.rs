//! An ARO session: one user namespace, one IPC namespace (hence one binder
//! instance), one mount namespace for the supervisor. App processes are
//! launched into it with their own mount and PID namespaces.
use anyhow::{Context, Result};
use aro_exec::layout::Layout;
use nix::mount::{mount, MsFlags};
use nix::sched::{unshare, CloneFlags};
use std::path::PathBuf;

pub struct Session {
    pub binderfs: PathBuf,
    pub sockets: PathBuf,
    /// Our real uid on the host. Inside the session we run as namespace-root
    /// (uid 0 -> this uid), so host services that authenticate by peer
    /// credentials see this uid; we present it to them explicitly.
    pub host_uid: u32,
}

/// Must be called while the process is still single-threaded.
pub fn enter(layout: &Layout) -> Result<Session> {
    let (uid, gid) = (nix::unistd::getuid().as_raw(), nix::unistd::getgid().as_raw());
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWIPC | CloneFlags::CLONE_NEWNS).context("unshare session namespaces")?;
    // Map to namespace-root: binderfs (and other filesystems) can only be
    // mounted in a user namespace that maps uid 0, and an unprivileged namespace
    // may map just one uid. uid 0 here still resolves to our real uid on the
    // host, so peer-credential checks outside the namespace see the real uid;
    // callers that assert an identity (D-Bus EXTERNAL) pass `host_uid`.
    std::fs::write("/proc/self/uid_map", format!("0 {uid} 1\n")).context("uid_map")?;
    std::fs::write("/proc/self/setgroups", "deny").context("setgroups")?;
    std::fs::write("/proc/self/gid_map", format!("0 {gid} 1\n")).context("gid_map")?;
    // Keep our mounts to ourselves and our children.
    mount(None::<&str>, "/", None::<&str>, MsFlags::MS_REC | MsFlags::MS_PRIVATE, None::<&str>).context("make / private")?;
    std::fs::create_dir_all(&layout.runtime)?;
    let binderfs = layout.runtime.join("binderfs");
    aro_exec::ns::mount_binderfs(&binderfs)?;
    let sockets = layout.runtime.join("socket");
    std::fs::create_dir_all(&sockets)?;
    Ok(Session { binderfs, sockets, host_uid: uid })
}
