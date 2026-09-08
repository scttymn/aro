//! `uimode`: android.app.IUiModeManager. Not published before, but an app with
//! a DayNight/night-mode theme constructs a `UiModeManager` during
//! `ResumeActivityItem` — `getServiceOrThrow("uimode")` would throw and abort
//! the resume (so the activity never reached RESUMED and never animated). A
//! minimal stub reporting a normal, non-night, unlocked mode is enough.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct UiModeService;

const UI_MODE_TYPE_NORMAL: i32 = 1; // Configuration.UI_MODE_TYPE_NORMAL
const MODE_NIGHT_NO: i32 = 1; // UiModeManager.MODE_NIGHT_NO
const MODE_NIGHT_CUSTOM_TYPE_UNKNOWN: i32 = -1;

impl Service for UiModeService {
    const DESCRIPTOR: &'static str = "android.app.IUiModeManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IUIMODEMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getCurrentModeType" => {
                ap::no_exception(reply)?;
                reply.write_i32(UI_MODE_TYPE_NORMAL)?;
                Ok(true)
            }
            "getNightMode" => {
                ap::no_exception(reply)?;
                reply.write_i32(MODE_NIGHT_NO)?;
                Ok(true)
            }
            "getNightModeCustomType" => {
                ap::no_exception(reply)?;
                reply.write_i32(MODE_NIGHT_CUSTOM_TYPE_UNKNOWN)?;
                Ok(true)
            }
            "getContrast" => {
                ap::no_exception(reply)?;
                reply.write_f32(0.0)?; // standard contrast
                Ok(true)
            }
            "isUiModeLocked" | "isNightModeLocked" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => {
                // addCallback and the setters are void; unknown getters get a
                // benign default. The client wraps these in try/catch(RemoteException).
                ap::no_exception(reply)?;
                if name.starts_with("is") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}
