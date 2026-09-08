//! `mount`: android.os.storage.IStorageManager. Reports one mounted primary
//! "external" volume — the host directory bound at /storage/emulated/0 (see
//! aro-exec, ARO_SDCARD). That's what StorageManager/Environment need to treat
//! /sdcard as usable storage; the bytes are the user's real files.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct StorageService;

const PRIMARY_PATH: &str = "/storage/emulated/0";

impl StorageService {
    /// android.os.storage.StorageVolume.writeToParcel.
    fn write_volume(p: &mut Parcel) -> Result<()> {
        ap::string8(p, Some("emulated;0"))?; // mId
        ap::string8(p, Some(PRIMARY_PATH))?; // mPath
        ap::string8(p, Some(PRIMARY_PATH))?; // mInternalPath
        ap::string8(p, Some("Internal shared storage"))?; // mDescription
        p.write_i32(1)?; // mPrimary
        p.write_i32(0)?; // mRemovable
        p.write_i32(1)?; // mEmulated
        p.write_i32(0)?; // mExternallyManaged
        p.write_i32(0)?; // mAllowMassStorage
        p.write_i64(0)?; // mMaxFileSize (0 = no limit)
        // mOwner: writeParcelable(UserHandle) = creator class name + body (userId).
        ap::string16(p, Some("android.os.UserHandle"))?;
        p.write_i32(0)?; // UserHandle.mHandle = user 0
        // storage UUID absent: flag 0 means the reader does NOT read a uuid string.
        p.write_i32(0)?;
        ap::string8(p, None)?; // mFsUuid (null for primary emulated)
        ap::string8(p, Some("mounted"))?; // mState == Environment.MEDIA_MOUNTED
        Ok(())
    }
}

impl Service for StorageService {
    const DESCRIPTOR: &'static str = "android.os.storage.IStorageManager";
    const TABLE: &'static [(u32, &'static str)] = super::storage_codes::ISTORAGEMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getVolumeList" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // array length
                reply.write_i32(1)?; // element present (readTypedObject flag)
                Self::write_volume(reply)?;
                Ok(true)
            }
            "getExternalStorageMountMode" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // MOUNT_MODE_EXTERNAL_DEFAULT
                Ok(true)
            }
            "getVolumes" | "getVolumeRecords" | "getDisks" => {
                // VolumeInfo[]/etc.: empty (StorageManager reads /sdcard via getVolumeList).
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "mkdirs" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("are") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
        }
    }
}
