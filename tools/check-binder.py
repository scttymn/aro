#!/usr/bin/env python3
"""Check private Binder setup independently of ARO, without root.

All namespace and mount changes happen in a child. Its mounts disappear on exit.
"""
import ctypes
import fcntl
import os
import struct
import tempfile


def check(mountpoint):
    libc = ctypes.CDLL(None, use_errno=True)
    uid, gid = os.getuid(), os.getgid()

    def syscall(result, operation):
        if result != 0:
            errno = ctypes.get_errno()
            raise OSError(errno, os.strerror(errno), operation)

    syscall(libc.unshare(0x10000000 | 0x08000000 | 0x00020000), "unshare user/ipc/mount")
    for path, value in [
        ("/proc/self/uid_map", f"0 {uid} 1\n"),
        ("/proc/self/setgroups", "deny"),
        ("/proc/self/gid_map", f"0 {gid} 1\n"),
    ]:
        with open(path, "w") as stream:
            stream.write(value)
    syscall(libc.mount(None, b"/", None, (1 << 18) | (1 << 14), None), "make mounts private")
    syscall(libc.mount(b"binder", os.fsencode(mountpoint), b"binder", 0, None), "mount binderfs")
    # BINDER_CTL_ADD = _IOWR('b', 1, struct binderfs_device).
    with open(os.path.join(mountpoint, "binder-control"), "rb", buffering=0) as control:
        device = bytearray(struct.pack("256sII", b"binder", 0, 0))
        fcntl.ioctl(control, 0xC1086201, device, True)
    print("binderfs mounted; binder device created", flush=True)
    with open(os.path.join(mountpoint, "binder"), "r+b", buffering=0) as binder:
        version = bytearray(4)
        # BINDER_VERSION = _IOWR('b', 9, struct binder_version).
        fcntl.ioctl(binder, 0xC0046209, version, True)
        print(f"Binder opened; protocol version {struct.unpack('i', version)[0]}", flush=True)


def main():
    print(f"Kernel: {os.uname().release}", flush=True)
    with tempfile.TemporaryDirectory(prefix="aro-binder-check-") as mountpoint:
        child = os.fork()
        if child == 0:
            try:
                check(mountpoint)
            except OSError as error:
                print(f"Binder check failed: {error}", flush=True)
                os._exit(1)
            os._exit(0)
        _, status = os.waitpid(child, 0)
        return os.waitstatus_to_exitcode(status)


if __name__ == "__main__":
    raise SystemExit(main())
