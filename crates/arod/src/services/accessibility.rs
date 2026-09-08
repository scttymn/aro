//! `accessibility`: android.view.accessibility.IAccessibilityManager (no services enabled).
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};

pub struct AccessibilityService;

impl Service for AccessibilityService {
    const DESCRIPTOR: &'static str = "android.view.accessibility.IAccessibilityManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IACCESSIBILITYMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "addClient" => {
                let _client: Option<SIBinder> = data.read()?;
                let _user = data.read_i32()?;
                ap::no_exception(reply)?;
                reply.write_i64(0)?; // no accessibility state flags
                Ok(true)
            }
            "removeClient" | "interrupt" | "sendAccessibilityEvent" => {
                ap::no_exception(reply).ok();
                Ok(true)
            }
            "getRecommendedTimeoutMillis" => {
                ap::no_exception(reply)?;
                reply.write_i64(0)?;
                Ok(true)
            }
            "getFocusStrokeWidth" | "getFocusColor" => {
                ap::no_exception(reply)?;
                reply.write_i32(if name == "getFocusColor" { 0xFF00_00FFu32 as i32 } else { 4 })?;
                Ok(true)
            }
            "isTouchExplorationEnabled" | "isAccessibilityServiceWarningRequired" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getEnabledAccessibilityServiceList" | "getInstalledAccessibilityServiceList" => {
                ap::no_exception(reply)?;
                ap::null_array(reply)?; // ParceledListSlice would be typed; null is tolerated by AccessibilityManager
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
