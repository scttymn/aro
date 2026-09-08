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

#[derive(Debug, Clone, Copy)]
pub struct BufferDesc {
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

#[derive(Default)]
pub struct Gralloc {
    pub buffers: Mutex<HashMap<u64, Arc<Buffer>>>,
    next_id: AtomicU64,
}

impl Gralloc {
    pub fn allocate(&self, desc: BufferDesc) -> Option<Arc<Buffer>> {
        let bpp = Buffer::bytes_per_pixel(desc.format)?;
        if desc.width == 0 || desc.height == 0 || desc.layer_count != 1 {
            return None;
        }
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
        let buf = Arc::new(Buffer { id, fd, desc, stride, size });
        log::info!("gralloc: buffer {id}: {}x{} fmt={} usage={:#x} stride={stride} size={size}", desc.width, desc.height, desc.format, desc.usage);
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
    if name_len > 0 {
        skip_aligned(data, name_len as usize);
    }
    let width = data.read_i32()? as u32;
    let height = data.read_i32()? as u32;
    let layer_count = data.read_i32()? as u32;
    let format = data.read_i32()?;
    let usage = data.read_i64()? as u64;
    let _reserved = data.read_i64().unwrap_or(0);
    data.set_data_position(start + size);
    Ok(Some(BufferDesc { width, height, layer_count, format, usage }))
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
                let ok = desc.map(|d| Buffer::bytes_per_pixel(d.format).is_some() && d.layer_count == 1).unwrap_or(false);
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
                    match self.gralloc.allocate(desc) {
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
