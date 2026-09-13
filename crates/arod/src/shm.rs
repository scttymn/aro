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
include!("../../../spec/system_features.rs");
/// `SystemFeaturesCache` insists on exactly this many entries for this build.
pub const SDK_FEATURE_COUNT: usize = SDK_FEATURES.len();
/// `SystemFeaturesCache.UNAVAILABLE_FEATURE_VERSION`.
pub const UNAVAILABLE_FEATURE_VERSION: i32 = i32::MIN;

pub fn application_shared_memory(versions: &[i32; SDK_FEATURE_COUNT]) -> Result<File> {
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
    for version in versions {
        features.extend_from_slice(&version.to_le_bytes());
    }
    file.write_all(&features)?;
    file.seek(SeekFrom::Start(FEATURE_COUNT_OFFSET))?;
    file.write_all(&(SDK_FEATURE_COUNT as i64).to_le_bytes())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::registry::{AppSpec, Registry};
    use std::io::Read;
    use std::sync::Mutex;

    #[test]
    fn framework_cache_tracks_provider_and_package_manager() {
        let registry = Registry { apps: Mutex::new(vec![]), display: (800, 600, 160) };
        // Index recovered from the image's hash-ordered SystemFeaturesMetadata.
        assert_eq!(SDK_FEATURES[134], "android.software.webview");
        assert_eq!(registry.system_feature_versions()[134], i32::MIN);
        let mut provider = AppSpec::default();
        provider.package = "com.android.webview".into();
        provider.meta_data.push(("com.android.webview.WebViewLibrary".into(), "libwebviewchromium.so".into()));
        registry.apps.lock().unwrap().push(provider);
        let versions = registry.system_feature_versions();
        let mut file = application_shared_memory(&versions).unwrap();
        file.seek(SeekFrom::Start(FEATURES_OFFSET)).unwrap();
        for name in SDK_FEATURES {
            let mut bytes = [0u8; 4];
            file.read_exact(&mut bytes).unwrap();
            assert_eq!(i32::from_le_bytes(bytes), registry.system_feature_version(name).unwrap_or(i32::MIN));
        }
        assert_eq!(versions[134], 0);
        file.seek(SeekFrom::Start(FEATURE_COUNT_OFFSET)).unwrap();
        let mut count = [0u8; 8];
        file.read_exact(&mut count).unwrap();
        assert_eq!(i64::from_le_bytes(count), 191);
    }

    #[test]
    fn feature_indices_follow_android_arrayset_hash_order() {
        let hashes: Vec<i32> = SDK_FEATURES.iter().map(|name| {
            name.encode_utf16().fold(0i32, |hash, c| hash.wrapping_mul(31).wrapping_add(i32::from(c)))
        }).collect();
        assert!(hashes.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
