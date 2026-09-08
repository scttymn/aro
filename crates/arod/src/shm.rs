//! Shared memory regions handed to apps.
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::FromRawFd;

/// Layout of `ApplicationSharedMemory` in this image (CP41.260814.003.B1),
/// recovered from `nativeMap`/`nativeReadSystemFeaturesCache` in
/// libandroid_runtime.so:
///   0x000  i64  latest network time (INVALID_NETWORK_TIME = -1)
///   0x008  f32  current animator duration scale
///   0x018  i32[512] SDK feature versions
///   0x818  i64  number of valid feature entries
///   total 0x2c38 bytes (nonce store and bit flags follow; zero is fine)
const SHM_SIZE: u64 = 0x2c38;
const FEATURES_OFFSET: u64 = 0x18;
const FEATURE_COUNT_OFFSET: u64 = 0x818;
/// `SystemFeaturesCache` insists on exactly this many entries for this build.
pub const SDK_FEATURE_COUNT: usize = 191;
/// `SystemFeaturesCache.UNAVAILABLE_FEATURE_VERSION`.
pub const UNAVAILABLE_FEATURE_VERSION: i32 = i32::MIN;

pub fn application_shared_memory() -> Result<File> {
    let name = std::ffi::CString::new("ApplicationSharedMemory")?;
    let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("memfd_create");
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.set_len(SHM_SIZE)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&(-1i64).to_le_bytes())?; // no network time
    file.write_all(&1.0f32.to_le_bytes())?; // animator scale
    file.seek(SeekFrom::Start(FEATURES_OFFSET))?;
    let mut features = Vec::with_capacity(SDK_FEATURE_COUNT * 4);
    for _ in 0..SDK_FEATURE_COUNT {
        features.extend_from_slice(&UNAVAILABLE_FEATURE_VERSION.to_le_bytes());
    }
    file.write_all(&features)?;
    file.seek(SeekFrom::Start(FEATURE_COUNT_OFFSET))?;
    file.write_all(&(SDK_FEATURE_COUNT as i64).to_le_bytes())?;
    Ok(file)
}
