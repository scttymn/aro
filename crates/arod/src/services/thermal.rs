//! `thermalservice`: android.os.IThermalService.
//! Minimal stub so PowerManager and other framework components don't fail
//! with ServiceNotFoundException.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct ThermalService;

impl Service for ThermalService {
    const DESCRIPTOR: &'static str = "android.os.IThermalService";
    const TABLE: &'static [(u32, &'static str)] = codes::ITHERMALSERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "registerThermalEventListener"
            | "registerThermalEventListenerWithType"
            | "unregisterThermalEventListener"
            | "registerThermalStatusListener"
            | "registerThermalStatusListenerForDevice"
            | "unregisterThermalStatusListener"
            | "registerThermalHeadroomListener"
            | "unregisterThermalHeadroomListener" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "getCurrentThermalStatus"
            | "getCurrentThermalStatusForDevice" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // THERMAL_STATUS_NONE
                Ok(true)
            }
            "getCurrentTemperatures"
            | "getCurrentTemperaturesWithType"
            | "getCurrentCoolingDevices"
            | "getCurrentCoolingDevicesWithType"
            | "getThermalHeadroomThresholds" => {
                ap::no_exception(reply)?;
                ap::empty_list(reply)?;
                Ok(true)
            }
            "getThermalHeadroom" => {
                ap::no_exception(reply)?;
                reply.write_f32(f32::NAN)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("register") || name.starts_with("unregister") || name.starts_with("is") {
                    ap::boolean(reply, true)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}
