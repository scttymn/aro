//! The ARO buffer handle: a `native_handle_t` with one fd (the memfd holding
//! the pixels) and a fixed set of ints describing the buffer. The allocator
//! (arod) writes it; this crate reads it. Keep both sides in sync.
use std::os::raw::c_int;

#[repr(C)]
pub struct native_handle_t {
    pub version: c_int,
    pub num_fds: c_int,
    pub num_ints: c_int,
    // data[num_fds + num_ints] follows
}

pub const MAGIC: i32 = 0x4152_4f42; // "AROB"
pub const NUM_FDS: usize = 1;
pub const NUM_INTS: usize = 11;

/// What the ints say. Same order as `arod::gralloc::HandleInts`.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub fd: c_int,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: i32,
    pub layer_count: u32,
    pub usage: u64,
    pub size: u32,
    pub id: u64,
}

impl Layout {
    pub unsafe fn from_handle(h: *const native_handle_t) -> Option<Layout> {
        if h.is_null() || (*h).version != std::mem::size_of::<native_handle_t>() as c_int || (*h).num_fds != NUM_FDS as c_int || (*h).num_ints != NUM_INTS as c_int {
            return None;
        }
        let data = (h as *const c_int).add(3);
        let ints = std::slice::from_raw_parts(data.add(NUM_FDS), NUM_INTS);
        if ints[0] != MAGIC {
            return None;
        }
        Some(Layout {
            fd: *data,
            width: ints[1] as u32,
            height: ints[2] as u32,
            stride: ints[3] as u32,
            format: ints[4],
            layer_count: ints[5] as u32,
            usage: (ints[6] as u32 as u64) | ((ints[7] as u32 as u64) << 32),
            size: ints[8] as u32,
            id: (ints[9] as u32 as u64) | ((ints[10] as u32 as u64) << 32),
        })
    }
}

/// native_handle_clone: same ints, dup'ed fds.
pub unsafe fn clone(h: *const native_handle_t) -> Option<*const native_handle_t> {
    let n = (*h).num_fds as usize + (*h).num_ints as usize;
    let bytes = std::mem::size_of::<native_handle_t>() + n * std::mem::size_of::<c_int>();
    let p = libc::malloc(bytes) as *mut native_handle_t;
    if p.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(h as *const u8, p as *mut u8, bytes);
    let data = (p as *mut c_int).add(3);
    for i in 0..(*h).num_fds as usize {
        let fd = libc::fcntl(*data.add(i), libc::F_DUPFD_CLOEXEC, 0);
        if fd < 0 {
            for j in 0..i {
                libc::close(*data.add(j));
            }
            libc::free(p as *mut _);
            return None;
        }
        *data.add(i) = fd;
    }
    Some(p)
}

pub unsafe fn close_and_free(h: *mut native_handle_t) {
    let data = (h as *mut c_int).add(3);
    for i in 0..(*h).num_fds as usize {
        libc::close(*data.add(i));
    }
    libc::free(h as *mut _);
}
