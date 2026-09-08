//! Syscall supervision for app processes (seccomp user notification).
//!
//! Some things Android expects from the kernel are privileges an unprivileged
//! user namespace cannot grant, such as raising a thread's priority (negative
//! nice). Android treats the refusal as fatal. The process host installs a
//! seccomp filter that routes those syscalls to the supervising parent, which
//! answers them itself; everything else runs unmodified.
use anyhow::{bail, Context, Result};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

// BPF and seccomp constants (linux/filter.h, linux/seccomp.h).
const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_USER_NOTIF: u32 = 0x7fc0_0000;
const SECCOMP_SET_MODE_FILTER: u32 = 1;
const SECCOMP_FILTER_FLAG_NEW_LISTENER: u32 = 1 << 3;
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
const SECCOMP_USER_NOTIF_FLAG_CONTINUE: u32 = 1;

#[repr(C)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SeccompData {
    nr: i32,
    arch: u32,
    instruction_pointer: u64,
    args: [u64; 6],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SeccompNotif {
    id: u64,
    pid: u32,
    flags: u32,
    data: SeccompData,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SeccompNotifResp {
    id: u64,
    val: i64,
    error: i32,
    flags: u32,
}

const fn ioctl_iowr(ty: u8, nr: u8, size: usize) -> u64 {
    (3u64 << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | nr as u64
}
const SECCOMP_IOCTL_NOTIF_RECV: u64 = ioctl_iowr(b'!', 0, std::mem::size_of::<SeccompNotif>());
const SECCOMP_IOCTL_NOTIF_SEND: u64 = ioctl_iowr(b'!', 1, std::mem::size_of::<SeccompNotifResp>());

/// Syscalls the supervisor handles.
const SUPERVISED: &[i64] = &[libc::SYS_setpriority];

/// Install the filter in the calling process (before exec). Returns the
/// listener fd the supervisor must service. Applies to all future threads
/// and children.
pub fn install() -> Result<OwnedFd> {
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error()).context("PR_SET_NO_NEW_PRIVS");
    }
    let mut prog: Vec<SockFilter> = Vec::new();
    // Only x86_64 for now: anything else is allowed through untouched.
    prog.push(SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 4 }); // arch
    prog.push(SockFilter { code: BPF_JMP_JEQ_K, jt: 1, jf: 0, k: AUDIT_ARCH_X86_64 });
    prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW });
    prog.push(SockFilter { code: BPF_LD_W_ABS, jt: 0, jf: 0, k: 0 }); // nr
    for nr in SUPERVISED {
        prog.push(SockFilter { code: BPF_JMP_JEQ_K, jt: 0, jf: 1, k: *nr as u32 });
        prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_USER_NOTIF });
    }
    prog.push(SockFilter { code: BPF_RET_K, jt: 0, jf: 0, k: SECCOMP_RET_ALLOW });
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    let fd = unsafe { libc::syscall(libc::SYS_seccomp, SECCOMP_SET_MODE_FILTER, SECCOMP_FILTER_FLAG_NEW_LISTENER, &fprog as *const SockFprog) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("seccomp(SET_MODE_FILTER, NEW_LISTENER)");
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

/// Service one pending notification on the listener. Call when it is readable.
/// Returns false when there was nothing to serve (the requester went away).
pub fn serve_one(listener: RawFd) -> Result<bool> {
    let mut req = SeccompNotif::default();
    if unsafe { libc::ioctl(listener, SECCOMP_IOCTL_NOTIF_RECV as _, &mut req as *mut SeccompNotif) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOENT) {
            return Ok(false); // the thread died before we answered
        }
        return Err(e).context("SECCOMP_IOCTL_NOTIF_RECV");
    }
    let mut resp = SeccompNotifResp { id: req.id, ..Default::default() };
    match req.data.nr as i64 {
        libc::SYS_setpriority => {
            let prio = req.data.args[2] as i32;
            if prio < 0 {
                // Pretend the raise succeeded; the thread simply keeps its current nice.
                resp.val = 0;
            } else {
                resp.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
            }
        }
        _ => resp.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE,
    }
    if unsafe { libc::ioctl(listener, SECCOMP_IOCTL_NOTIF_SEND as _, &resp as *const SeccompNotifResp) } != 0 {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(libc::ENOENT) {
            return Ok(true);
        }
        return Err(e).context("SECCOMP_IOCTL_NOTIF_SEND");
    }
    Ok(true)
}

/// Send an fd over a Unix socket (child -> parent).
pub fn send_fd(sock: RawFd, fd: RawFd) -> Result<()> {
    use nix::sys::socket::{sendmsg, ControlMessage, MsgFlags};
    use std::io::IoSlice;
    let fds = [fd];
    let cmsg = [ControlMessage::ScmRights(&fds)];
    sendmsg::<()>(sock, &[IoSlice::new(b"F")], &cmsg, MsgFlags::empty(), None).context("sendmsg(SCM_RIGHTS)")?;
    Ok(())
}

/// Receive one fd over a Unix socket (parent side).
pub fn recv_fd(sock: RawFd) -> Result<OwnedFd> {
    use nix::sys::socket::{recvmsg, ControlMessageOwned, MsgFlags};
    use std::io::IoSliceMut;
    let mut buf = [0u8; 1];
    let mut iov = [IoSliceMut::new(&mut buf)];
    let mut cmsg = nix::cmsg_space!([RawFd; 1]);
    let msg = recvmsg::<()>(sock, &mut iov, Some(&mut cmsg), MsgFlags::empty()).context("recvmsg(SCM_RIGHTS)")?;
    for c in msg.cmsgs()? {
        if let ControlMessageOwned::ScmRights(fds) = c {
            if let Some(fd) = fds.first() {
                return Ok(unsafe { OwnedFd::from_raw_fd(*fd) });
            }
        }
    }
    bail!("no fd received from child")
}
