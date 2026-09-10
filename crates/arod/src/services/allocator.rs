//! ARO gralloc, allocator half: `android.hardware.graphics.allocator.IAllocator/default`
//! (AIDL, @VintfStability). Buffers are memfds; the native handle carries one
//! fd and the ints in `HandleInts` order, which `crates/aro-mapper` decodes
//! inside the app. arod keeps every buffer so the composer can map the pixels
//! when the app queues them.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const HANDLE_MAGIC: i32 = 0x4152_4f42; // "AROB"
pub const MAPPER_SUFFIX: &str = "aro";

#[derive(Debug, Clone)]
pub struct BufferDesc {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub layer_count: u32,
    pub format: i32,
    pub usage: u64,
}

pub struct Buffer {
    pub id: u64,
    pub fd: OwnedFd,
    pub desc: BufferDesc,
    pub stride: u32,
    pub size: u32,
    pub is_gbm: bool,
    pub modifier: u64,
}

impl Buffer {
    pub fn bytes_per_pixel(format: i32) -> Option<u32> {
        match format {
            1 | 2 | 5 => Some(4), // RGBA_8888, RGBX_8888, BGRA_8888
            3 => Some(3),         // RGB_888
            4 => Some(2),         // RGB_565
            _ => None,
        }
    }

    /// Handle ints, in the order aro-mapper's `handle::Layout` reads them.
    pub fn handle_ints(&self) -> [i32; 11] {
        [
            HANDLE_MAGIC,
            self.desc.width as i32,
            self.desc.height as i32,
            self.stride as i32,
            self.desc.format,
            self.desc.layer_count as i32,
            self.desc.usage as u32 as i32,
            (self.desc.usage >> 32) as u32 as i32,
            self.size as i32,
            self.id as u32 as i32,
            (self.id >> 32) as u32 as i32,
        ]
    }
}

struct GbmDevice {
    _lib: *mut libc::c_void,
    dev: *mut libc::c_void,
    _dri_fd: OwnedFd,
    destroy: unsafe extern "C" fn(*mut libc::c_void),
    bo_create: unsafe extern "C" fn(*mut libc::c_void, u32, u32, u32, u32) -> *mut libc::c_void,
    bo_destroy: unsafe extern "C" fn(*mut libc::c_void),
    bo_get_stride: unsafe extern "C" fn(*mut libc::c_void) -> u32,
    bo_get_fd: unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int,
    bo_get_modifier: unsafe extern "C" fn(*mut libc::c_void) -> u64,
}
unsafe impl Send for GbmDevice {}
unsafe impl Sync for GbmDevice {}

impl Drop for GbmDevice {
    fn drop(&mut self) {
        if !self.dev.is_null() {
            unsafe { (self.destroy)(self.dev) };
        }
        if !self._lib.is_null() {
            unsafe { libc::dlclose(self._lib) };
        }
    }
}

fn find_drm_render_node() -> Option<std::path::PathBuf> {
    if let Some(val) = std::env::var_os("ARO_RENDER_NODE").or_else(|| std::env::var_os("ARO_DRI_NODE")) {
        let p = std::path::PathBuf::from(val);
        if p.exists() {
            return Some(p);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("renderD") {
                let path = entry.path();
                let sys_vendor_path = format!("/sys/class/drm/{}/device/vendor", name_str);
                let vendor = std::fs::read_to_string(&sys_vendor_path)
                    .ok()
                    .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
                    .unwrap_or(0);
                candidates.push((path, vendor));
            }
        }
    }
    // Prefer Intel (0x8086) for anv Vulkan HAL, otherwise first available
    candidates.sort_by_key(|(_, v)| if *v == 0x8086 { 0 } else { 1 });
    candidates.into_iter().next().map(|(p, _)| p)
}

impl GbmDevice {
    fn try_new() -> Option<Self> {
        let path = find_drm_render_node()?;
        let c_path = std::ffi::CString::new(path.to_str()?).ok()?;
        let raw_fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if raw_fd < 0 {
            log::warn!("gralloc: failed to open DRI node {}: {}", path.display(), std::io::Error::last_os_error());
            return None;
        }
        let dri_fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };

        let lib_names = [c"libgbm.so.1", c"libgbm.so"];
        let mut lib = std::ptr::null_mut();
        for &name in &lib_names {
            lib = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW) };
            if !lib.is_null() {
                break;
            }
        }
        if lib.is_null() {
            log::warn!("gralloc: failed to dlopen libgbm");
            return None;
        }

        unsafe {
            let create_dev: unsafe extern "C" fn(libc::c_int) -> *mut libc::c_void =
                std::mem::transmute(libc::dlsym(lib, c"gbm_create_device".as_ptr()));
            let destroy: unsafe extern "C" fn(*mut libc::c_void) =
                std::mem::transmute(libc::dlsym(lib, c"gbm_device_destroy".as_ptr()));
            let bo_create: unsafe extern "C" fn(*mut libc::c_void, u32, u32, u32, u32) -> *mut libc::c_void =
                std::mem::transmute(libc::dlsym(lib, c"gbm_bo_create".as_ptr()));
            let bo_destroy: unsafe extern "C" fn(*mut libc::c_void) =
                std::mem::transmute(libc::dlsym(lib, c"gbm_bo_destroy".as_ptr()));
            let bo_get_stride: unsafe extern "C" fn(*mut libc::c_void) -> u32 =
                std::mem::transmute(libc::dlsym(lib, c"gbm_bo_get_stride".as_ptr()));
            let bo_get_fd: unsafe extern "C" fn(*mut libc::c_void) -> libc::c_int =
                std::mem::transmute(libc::dlsym(lib, c"gbm_bo_get_fd".as_ptr()));
            let bo_get_modifier: unsafe extern "C" fn(*mut libc::c_void) -> u64 =
                std::mem::transmute(libc::dlsym(lib, c"gbm_bo_get_modifier".as_ptr()));

            let dev = create_dev(dri_fd.as_raw_fd());
            if dev.is_null() {
                log::warn!("gralloc: gbm_create_device failed on {}", path.display());
                libc::dlclose(lib);
                return None;
            }
            log::info!("gralloc: initialized GBM device on {}", path.display());
            Some(GbmDevice {
                _lib: lib,
                dev,
                _dri_fd: dri_fd,
                destroy,
                bo_create,
                bo_destroy,
                bo_get_stride,
                bo_get_fd,
                bo_get_modifier,
            })
        }
    }
}

fn hal_format_to_gbm(format: i32) -> Option<u32> {
    match format {
        1 => Some(0x34324241), // RGBA_8888 -> DRM_FORMAT_ABGR8888 ('AB24')
        2 => Some(0x34324258), // RGBX_8888 -> DRM_FORMAT_XBGR8888 ('XB24')
        3 => Some(0x34324752), // RGB_888   -> DRM_FORMAT_RGB888   ('RG24')
        4 => Some(0x36314752), // RGB_565   -> DRM_FORMAT_RGB565   ('RG16')
        5 => Some(0x34325241), // BGRA_8888 -> DRM_FORMAT_ARGB8888 ('AR24')
        _ => None,
    }
}

pub struct Gralloc {
    gbm: Option<GbmDevice>,
    pub buffers: Mutex<HashMap<u64, Arc<Buffer>>>,
    next_id: AtomicU64,
}

impl Default for Gralloc {
    fn default() -> Self {
        Gralloc {
            gbm: GbmDevice::try_new(),
            buffers: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
        }
    }
}

impl Gralloc {
    pub fn allocate(&self, desc: BufferDesc) -> Option<Arc<Buffer>> {
        let bpp = Buffer::bytes_per_pixel(desc.format)?;
        if desc.width == 0 || desc.height == 0 || desc.layer_count != 1 {
            return None;
        }

        // 1. Try GBM hardware BO allocation
        if let Some(gbm) = &self.gbm {
            if let Some(gbm_fmt) = hal_format_to_gbm(desc.format) {
                // Try SCANOUT | RENDERING (0x5), fall back to RENDERING (0x4), then 0
                let mut bo = unsafe { (gbm.bo_create)(gbm.dev, desc.width, desc.height, gbm_fmt, 5) };
                if bo.is_null() {
                    bo = unsafe { (gbm.bo_create)(gbm.dev, desc.width, desc.height, gbm_fmt, 4) };
                }
                if bo.is_null() {
                    bo = unsafe { (gbm.bo_create)(gbm.dev, desc.width, desc.height, gbm_fmt, 0) };
                }
                if !bo.is_null() {
                    let stride_bytes = unsafe { (gbm.bo_get_stride)(bo) };
                    let stride = stride_bytes / bpp;
                    let modifier = unsafe { (gbm.bo_get_modifier)(bo) };
                    let raw_fd = unsafe { (gbm.bo_get_fd)(bo) };
                    unsafe { (gbm.bo_destroy)(bo) };
                    if raw_fd >= 0 {
                        let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
                        let mut st: libc::stat = unsafe { std::mem::zeroed() };
                        let size = if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0 && st.st_size > 0 {
                            st.st_size as u32
                        } else {
                            (stride_bytes * desc.height + 4095) & !4095
                        };
                        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
                        log::info!("gralloc: gbm bo {id} ({:?}): {}x{} fmt={} usage={:#x} stride={stride} size={size} mod={modifier:#x}", desc.name, desc.width, desc.height, desc.format, desc.usage);
                        let buf = Arc::new(Buffer { id, fd, desc, stride, size, is_gbm: true, modifier });
                        self.buffers.lock().unwrap().insert(id, buf.clone());
                        return Some(buf);
                    }
                } else {
                    log::warn!("gralloc: gbm_bo_create failed for {}x{} fmt={}", desc.width, desc.height, desc.format);
                }
            }
        }

        // 2. Fallback to memfd
        let stride = (desc.width + 63) & !63;
        let size = (stride * bpp * desc.height + 4095) & !4095;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let name = std::ffi::CString::new(format!("aro-buffer-{id}")).ok()?;
        let raw = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
        if raw < 0 {
            log::error!("gralloc: memfd_create failed: {}", std::io::Error::last_os_error());
            return None;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
            log::error!("gralloc: ftruncate failed: {}", std::io::Error::last_os_error());
            return None;
        }
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_ADD_SEALS, libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL) };
        log::info!("gralloc: memfd buffer {id} ({:?}): {}x{} fmt={} usage={:#x} stride={stride} size={size}", desc.name, desc.width, desc.height, desc.format, desc.usage);
        let buf = Arc::new(Buffer { id, fd, desc, stride, size, is_gbm: false, modifier: 0 });
        self.buffers.lock().unwrap().insert(id, buf.clone());
        Some(buf)
    }
}

/// android.hardware.graphics.allocator.IAllocator (AIDL declaration order).
pub static IALLOCATOR: &[(u32, &str)] = &[
    (1, "allocate"),
    (2, "allocate2"),
    (3, "isSupported"),
    (4, "getIMapperLibrarySuffix"),
    (5, "isMultiViewSupported"),
    (6, "allocateMultiView"),
    (0x00ff_ffff, "getInterfaceVersion"),
    (0x00ff_fffe, "getInterfaceHash"),
];

pub struct AllocatorService {
    pub gralloc: Arc<Gralloc>,
}

fn skip_aligned(data: &mut Parcel, bytes: usize) {
    let pos = data.data_position();
    data.set_data_position(pos + ((bytes + 3) & !3));
}

/// NDK BufferDescriptorInfo: [nonnull][size][byte[128] name][w][h][layers][format][usage i64][reserved i64][ExtendableType[]]
fn read_descriptor(data: &mut Parcel) -> Result<Option<BufferDesc>> {
    if data.read_i32()? == 0 {
        return Ok(None);
    }
    let start = data.data_position();
    let size = data.read_i32()? as usize;
    let name_len = data.read_i32()?;
    let name = if name_len > 0 {
        let pos = data.data_position();
        let (bytes, _) = data.aro_debug_bytes();
        let s = if pos + name_len as usize <= bytes.len() {
            String::from_utf8_lossy(&bytes[pos..pos + name_len as usize]).trim_matches('\0').to_string()
        } else {
            String::new()
        };
        skip_aligned(data, name_len as usize);
        s
    } else {
        String::new()
    };
    let width = data.read_i32()? as u32;
    let height = data.read_i32()? as u32;
    let layer_count = data.read_i32()? as u32;
    let format = data.read_i32()?;
    let usage = data.read_i64()? as u64;
    let _reserved = data.read_i64().unwrap_or(0);
    data.set_data_position(start + size);
    Ok(Some(BufferDesc { name, width, height, layer_count, format, usage }))
}

/// android.hardware.common.NativeHandle (NDK): [nonnull][size][fds: n, each nonnull+hasComm+fd][ints: n + values]
fn write_native_handle(p: &mut Parcel, buf: &Buffer) -> Result<()> {
    p.write_i32(1)?;
    let start = p.data_position();
    p.write_i32(0)?;
    p.write_i32(1)?; // one fd
    p.write_i32(1)?; // ParcelFileDescriptor non-null
    p.write_i32(0)?; // hasComm
    p.write_raw_file_descriptor(buf.fd.as_fd())?;
    let ints = buf.handle_ints();
    p.write_i32(ints.len() as i32)?;
    for i in ints {
        p.write_i32(i)?;
    }
    let end = p.data_position();
    p.set_data_position(start);
    p.write_i32((end - start) as i32)?;
    p.set_data_position(end);
    Ok(())
}

impl Service for AllocatorService {
    const DESCRIPTOR: &'static str = "android.hardware.graphics.allocator.IAllocator";
    const TABLE: &'static [(u32, &'static str)] = IALLOCATOR;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getInterfaceVersion" => {
                ap::no_exception(reply)?;
                reply.write_i32(3)?;
                Ok(true)
            }
            "getInterfaceHash" => {
                ap::no_exception(reply)?;
                ap::string16(reply, Some("notfrozen"))?;
                Ok(true)
            }
            "getIMapperLibrarySuffix" => {
                ap::no_exception(reply)?;
                ap::string16(reply, Some(MAPPER_SUFFIX))?;
                Ok(true)
            }
            "isSupported" => {
                let desc = read_descriptor(data)?;
                let ok = desc.as_ref().map(|d| Buffer::bytes_per_pixel(d.format).is_some() && d.layer_count == 1).unwrap_or(false);
                log::info!("gralloc: isSupported {desc:?} -> {ok}");
                ap::no_exception(reply)?;
                ap::boolean(reply, ok)?;
                Ok(true)
            }
            "allocate2" => {
                let Some(desc) = read_descriptor(data)? else { return Err(rsbinder::StatusCode::BadValue) };
                let count = data.read_i32()?.max(0) as usize;
                let mut bufs = Vec::with_capacity(count);
                for _ in 0..count {
                    match self.gralloc.allocate(desc.clone()) {
                        Some(b) => bufs.push(b),
                        None => {
                            log::warn!("gralloc: unsupported allocation {desc:?}");
                            // EX_SERVICE_SPECIFIC with AllocationError::UNSUPPORTED (2)
                            reply.write_i32(-8)?;
                            ap::string16(reply, Some("unsupported"))?;
                            reply.write_i32(2)?;
                            return Ok(true);
                        }
                    }
                }
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // AllocationResult non-null
                let start = reply.data_position();
                reply.write_i32(0)?;
                reply.write_i32(bufs.first().map(|b| b.stride as i32).unwrap_or(0))?;
                reply.write_i32(bufs.len() as i32)?;
                for b in &bufs {
                    write_native_handle(reply, b)?;
                }
                let end = reply.data_position();
                reply.set_data_position(start);
                reply.write_i32((end - start) as i32)?;
                reply.set_data_position(end);
                Ok(true)
            }
            "isMultiViewSupported" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
