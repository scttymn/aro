//! ARO's gralloc mapper: the `mapper.aro.so` that libui dlopens inside every
//! app process (stable-C `AIMapper` v5). Buffers are plain shared memory
//! (a memfd per buffer) described by the native handle the ARO allocator
//! hands out; see `handle::Layout` for the ints.
//!
//! Built for `x86_64-linux-android` (bionic) and linked with the NDK, since
//! it runs in the app's namespace, not in arod.
#![allow(non_camel_case_types, clippy::missing_safety_doc)]

mod handle;
mod metadata;

use handle::{native_handle_t, Layout};
use std::collections::HashMap;
use std::sync::Mutex;

pub type buffer_handle_t = *const native_handle_t;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ARect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct AIMapper_MetadataType {
    pub name: *const libc::c_char,
    pub value: i64,
}

#[repr(C)]
pub struct AIMapper_MetadataTypeDescription {
    pub metadata_type: AIMapper_MetadataType,
    pub description: *const libc::c_char,
    pub is_gettable: bool,
    pub is_settable: bool,
    pub reserved: [u8; 32],
}

pub type AIMapper_Error = i32;
pub const ERROR_NONE: AIMapper_Error = 0;
pub const ERROR_BAD_BUFFER: AIMapper_Error = 2;
pub const ERROR_BAD_VALUE: AIMapper_Error = 3;
pub const ERROR_NO_RESOURCES: AIMapper_Error = 5;
pub const ERROR_UNSUPPORTED: AIMapper_Error = 7;

pub type DumpBufferCallback = unsafe extern "C" fn(*mut libc::c_void, AIMapper_MetadataType, *const libc::c_void, usize);
pub type BeginDumpBufferCallback = unsafe extern "C" fn(*mut libc::c_void);

#[repr(C)]
pub struct AIMapperV5 {
    pub import_buffer: unsafe extern "C" fn(*const native_handle_t, *mut buffer_handle_t) -> AIMapper_Error,
    pub free_buffer: unsafe extern "C" fn(buffer_handle_t) -> AIMapper_Error,
    pub get_transport_size: unsafe extern "C" fn(buffer_handle_t, *mut u32, *mut u32) -> AIMapper_Error,
    pub lock: unsafe extern "C" fn(buffer_handle_t, u64, ARect, libc::c_int, *mut *mut libc::c_void) -> AIMapper_Error,
    pub unlock: unsafe extern "C" fn(buffer_handle_t, *mut libc::c_int) -> AIMapper_Error,
    pub flush_locked_buffer: unsafe extern "C" fn(buffer_handle_t) -> AIMapper_Error,
    pub reread_locked_buffer: unsafe extern "C" fn(buffer_handle_t) -> AIMapper_Error,
    pub get_metadata: unsafe extern "C" fn(buffer_handle_t, AIMapper_MetadataType, *mut libc::c_void, usize) -> i32,
    pub get_standard_metadata: unsafe extern "C" fn(buffer_handle_t, i64, *mut libc::c_void, usize) -> i32,
    pub set_metadata: unsafe extern "C" fn(buffer_handle_t, AIMapper_MetadataType, *const libc::c_void, usize) -> AIMapper_Error,
    pub set_standard_metadata: unsafe extern "C" fn(buffer_handle_t, i64, *const libc::c_void, usize) -> AIMapper_Error,
    pub list_supported_metadata_types: unsafe extern "C" fn(*mut *const AIMapper_MetadataTypeDescription, *mut usize) -> AIMapper_Error,
    pub dump_buffer: unsafe extern "C" fn(buffer_handle_t, DumpBufferCallback, *mut libc::c_void) -> AIMapper_Error,
    pub dump_all_buffers: unsafe extern "C" fn(BeginDumpBufferCallback, DumpBufferCallback, *mut libc::c_void) -> AIMapper_Error,
    pub get_reserved_region: unsafe extern "C" fn(buffer_handle_t, *mut *mut libc::c_void, *mut u64) -> AIMapper_Error,
}

#[repr(C)]
pub struct AIMapperV6 {
    pub get_multi_view_info: unsafe extern "C" fn(buffer_handle_t, *mut *const u32, *mut usize) -> AIMapper_Error,
    pub get_base_view: unsafe extern "C" fn(buffer_handle_t, *mut u32) -> AIMapper_Error,
    pub import_view_buffer: unsafe extern "C" fn(buffer_handle_t, u32, *mut buffer_handle_t) -> AIMapper_Error,
}

#[repr(C, align(16))]
pub struct AIMapper {
    pub version: u32,
    pub v5: AIMapperV5,
    pub v6: AIMapperV6,
}

/// An imported buffer: our own copy of the handle plus the CPU mapping.
struct Imported {
    layout: Layout,
    mapping: *mut u8,
}
unsafe impl Send for Imported {}

static BUFFERS: Mutex<Option<HashMap<usize, Imported>>> = Mutex::new(None);

fn with_buffers<R>(f: impl FnOnce(&mut HashMap<usize, Imported>) -> R) -> R {
    let mut g = BUFFERS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new))
}

unsafe extern "C" fn import_buffer(raw: *const native_handle_t, out: *mut buffer_handle_t) -> AIMapper_Error {
    let Some(layout) = Layout::from_handle(raw) else { return ERROR_BAD_BUFFER };
    let Some(cloned) = handle::clone(raw) else { return ERROR_NO_RESOURCES };
    let layout = Layout::from_handle(cloned).unwrap_or(layout);
    with_buffers(|b| b.insert(cloned as usize, Imported { layout, mapping: std::ptr::null_mut() }));
    *out = cloned;
    ERROR_NONE
}

unsafe extern "C" fn free_buffer(buffer: buffer_handle_t) -> AIMapper_Error {
    let Some(imp) = with_buffers(|b| b.remove(&(buffer as usize))) else { return ERROR_BAD_BUFFER };
    if !imp.mapping.is_null() {
        libc::munmap(imp.mapping as *mut _, imp.layout.size as usize);
    }
    handle::close_and_free(buffer as *mut native_handle_t);
    ERROR_NONE
}

unsafe extern "C" fn get_transport_size(buffer: buffer_handle_t, num_fds: *mut u32, num_ints: *mut u32) -> AIMapper_Error {
    if buffer.is_null() {
        return ERROR_BAD_BUFFER;
    }
    *num_fds = (*buffer).num_fds as u32;
    *num_ints = (*buffer).num_ints as u32;
    ERROR_NONE
}

unsafe extern "C" fn lock(buffer: buffer_handle_t, _cpu_usage: u64, _region: ARect, acquire_fence: libc::c_int, out: *mut *mut libc::c_void) -> AIMapper_Error {
    if acquire_fence >= 0 {
        // Software buffers: there is nothing to wait for, but honour the fd.
        libc::close(acquire_fence);
    }
    let r = with_buffers(|b| {
        let Some(imp) = b.get_mut(&(buffer as usize)) else { return Err(ERROR_BAD_BUFFER) };
        if imp.mapping.is_null() {
            let p = libc::mmap(std::ptr::null_mut(), imp.layout.size as usize, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, imp.layout.fd, 0);
            if p == libc::MAP_FAILED {
                return Err(ERROR_NO_RESOURCES);
            }
            imp.mapping = p as *mut u8;
        }
        Ok(imp.mapping)
    });
    match r {
        Ok(p) => {
            *out = p as *mut _;
            ERROR_NONE
        }
        Err(e) => e,
    }
}

unsafe extern "C" fn unlock(buffer: buffer_handle_t, release_fence: *mut libc::c_int) -> AIMapper_Error {
    if !with_buffers(|b| b.contains_key(&(buffer as usize))) {
        return ERROR_BAD_BUFFER;
    }
    *release_fence = -1;
    ERROR_NONE
}

unsafe extern "C" fn flush_locked_buffer(_buffer: buffer_handle_t) -> AIMapper_Error {
    ERROR_NONE
}
unsafe extern "C" fn reread_locked_buffer(_buffer: buffer_handle_t) -> AIMapper_Error {
    ERROR_NONE
}

unsafe extern "C" fn get_metadata(buffer: buffer_handle_t, ty: AIMapper_MetadataType, dest: *mut libc::c_void, dest_size: usize) -> i32 {
    if ty.name.is_null() || std::ffi::CStr::from_ptr(ty.name).to_bytes() != metadata::STANDARD_TYPE.as_bytes() {
        return -ERROR_UNSUPPORTED;
    }
    get_standard_metadata(buffer, ty.value, dest, dest_size)
}

unsafe extern "C" fn get_standard_metadata(buffer: buffer_handle_t, ty: i64, dest: *mut libc::c_void, dest_size: usize) -> i32 {
    let Some(layout) = with_buffers(|b| b.get(&(buffer as usize)).map(|i| i.layout)) else { return -ERROR_BAD_BUFFER };
    let Some(blob) = metadata::encode(&layout, ty) else { return -ERROR_UNSUPPORTED };
    if !dest.is_null() && dest_size >= blob.len() {
        std::ptr::copy_nonoverlapping(blob.as_ptr(), dest as *mut u8, blob.len());
    }
    blob.len() as i32
}

unsafe extern "C" fn set_metadata(_b: buffer_handle_t, _t: AIMapper_MetadataType, _d: *const libc::c_void, _s: usize) -> AIMapper_Error {
    ERROR_UNSUPPORTED
}
unsafe extern "C" fn set_standard_metadata(_b: buffer_handle_t, ty: i64, _d: *const libc::c_void, _s: usize) -> AIMapper_Error {
    // Dataspace / blend mode / HDR metadata writes are accepted and ignored for now.
    match ty {
        metadata::DATASPACE | metadata::BLEND_MODE | metadata::SMPTE2086 | metadata::CTA861_3 | metadata::SMPTE2094_40 | metadata::SMPTE2094_10 | metadata::CROP => ERROR_NONE,
        _ => ERROR_UNSUPPORTED,
    }
}

unsafe extern "C" fn list_supported_metadata_types(out: *mut *const AIMapper_MetadataTypeDescription, n: *mut usize) -> AIMapper_Error {
    *out = metadata::DESCRIPTIONS.as_ptr();
    *n = metadata::DESCRIPTIONS.len();
    ERROR_NONE
}

unsafe extern "C" fn dump_buffer(buffer: buffer_handle_t, cb: DumpBufferCallback, ctx: *mut libc::c_void) -> AIMapper_Error {
    let Some(layout) = with_buffers(|b| b.get(&(buffer as usize)).map(|i| i.layout)) else { return ERROR_BAD_BUFFER };
    for d in metadata::DESCRIPTIONS.iter() {
        if let Some(blob) = metadata::encode(&layout, d.metadata_type.value) {
            cb(ctx, AIMapper_MetadataType { name: d.metadata_type.name, value: d.metadata_type.value }, blob.as_ptr() as *const _, blob.len());
        }
    }
    ERROR_NONE
}

unsafe extern "C" fn dump_all_buffers(begin: BeginDumpBufferCallback, cb: DumpBufferCallback, ctx: *mut libc::c_void) -> AIMapper_Error {
    let layouts: Vec<Layout> = with_buffers(|b| b.values().map(|i| i.layout).collect());
    for layout in layouts {
        begin(ctx);
        for d in metadata::DESCRIPTIONS.iter() {
            if let Some(blob) = metadata::encode(&layout, d.metadata_type.value) {
                cb(ctx, AIMapper_MetadataType { name: d.metadata_type.name, value: d.metadata_type.value }, blob.as_ptr() as *const _, blob.len());
            }
        }
    }
    ERROR_NONE
}

unsafe extern "C" fn get_reserved_region(_b: buffer_handle_t, out: *mut *mut libc::c_void, size: *mut u64) -> AIMapper_Error {
    *out = std::ptr::null_mut();
    *size = 0;
    ERROR_NONE
}

unsafe extern "C" fn get_multi_view_info(_b: buffer_handle_t, _l: *mut *const u32, _n: *mut usize) -> AIMapper_Error {
    ERROR_UNSUPPORTED
}
unsafe extern "C" fn get_base_view(_b: buffer_handle_t, _v: *mut u32) -> AIMapper_Error {
    ERROR_UNSUPPORTED
}
unsafe extern "C" fn import_view_buffer(_b: buffer_handle_t, _v: u32, _o: *mut buffer_handle_t) -> AIMapper_Error {
    ERROR_UNSUPPORTED
}

static MAPPER: AIMapper = AIMapper {
    version: 5,
    v5: AIMapperV5 {
        import_buffer,
        free_buffer,
        get_transport_size,
        lock,
        unlock,
        flush_locked_buffer,
        reread_locked_buffer,
        get_metadata,
        get_standard_metadata,
        set_metadata,
        set_standard_metadata,
        list_supported_metadata_types,
        dump_buffer,
        dump_all_buffers,
        get_reserved_region,
    },
    v6: AIMapperV6 { get_multi_view_info, get_base_view, import_view_buffer },
};

/// The entry point libui looks up with dlsym.
#[no_mangle]
pub unsafe extern "C" fn AIMapper_loadIMapper(out: *mut *const AIMapper) -> AIMapper_Error {
    *out = &MAPPER;
    ERROR_NONE
}
