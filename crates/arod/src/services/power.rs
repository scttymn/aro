//! `power`: android.os.IPowerManager.
//! Minimal stub so PowerManager (wake locks, interactive state) doesn't throw
//! ServiceNotFoundException when an app or service requests wake locks.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct PowerService;

impl Service for PowerService {
    const DESCRIPTOR: &'static str = "android.os.IPowerManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IPOWERMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "acquireWakeLock"
            | "acquireWakeLockWithUid"
            | "releaseWakeLock"
            | "updateWakeLockUids"
            | "updateWakeLockWorkSource"
            | "userActivity" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "isWakeLockLevelSupported"
            | "isWakeLockLevelSupportedWithDisplayId"
            | "isInteractive"
            | "isDisplayInteractive" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "isPowerSaveMode"
            | "isDeviceIdleMode"
            | "isLightDeviceIdleMode"
            | "areAutoPowerSaveModesEnabled"
            | "isBatterySaverSupported"
            | "isLowPowerStandbyEnabled" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("has") || name.starts_with("can") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}
