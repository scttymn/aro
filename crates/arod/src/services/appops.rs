//! `appops`: com.android.internal.app.IAppOpsService.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct AppOpsService;

const MODE_ALLOWED: i32 = 0; // AppOpsManager.MODE_ALLOWED

impl Service for AppOpsService {
    const DESCRIPTOR: &'static str = "com.android.internal.app.IAppOpsService";
    const TABLE: &'static [(u32, &'static str)] = codes::IAPPOPSSERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "checkOperation"
            | "checkAudioOperation"
            | "checkOperationForDevice"
            | "checkPackage"
            | "checkOperationRaw" => {
                ap::no_exception(reply)?;
                reply.write_i32(MODE_ALLOWED)?;
                Ok(true)
            }
            "permissionToOpCode" => {
                ap::no_exception(reply)?;
                reply.write_i32(-1)?; // OP_NONE
                Ok(true)
            }
            "noteOperation" | "noteProxyOperation" | "noteProxyOperationWithState" => {
                ap::no_exception(reply)?;
                // SyncNotedAppOp
                ap::typed(reply, |p| {
                    p.write_i8(0)?;
                    p.write_i32(MODE_ALLOWED)?;
                    p.write_i32(0)?;
                    ap::string16(p, None)?;
                    ap::string16(p, None)
                })?;
                Ok(true)
            }
            "startOperation" | "startProxyOperation" => {
                ap::no_exception(reply)?;
                ap::typed(reply, |p| {
                    p.write_i8(0)?;
                    p.write_i32(MODE_ALLOWED)?;
                    p.write_i32(0)?;
                    ap::string16(p, None)?;
                    ap::string16(p, None)
                })?;
                Ok(true)
            }
            "finishOperation" | "finishProxyOperation" | "startWatchingMode" | "stopWatchingMode" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "shouldCollectNotes" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("should") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") || name.starts_with("check") {
                    reply.write_i32(MODE_ALLOWED)?;
                }
                Ok(true)
            }
        }
    }
}
