//! `sensorservice`: android.gui.SensorServer (a hand-written C++ Binder
//! interface, libsensor's ISensorServer.cpp). Replies carry no exception
//! header. ARO reports no sensors for now; a real backend can map desktop
//! sensors (iio) later.
use super::Service;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};

pub struct SensorService;

/// ISensorServer transaction codes (FIRST_CALL_TRANSACTION + index).
pub static ISENSORSERVER: &[(u32, &str)] = &[
    (1, "getSensorList"),
    (2, "createSensorEventConnection"),
    (3, "enableDataInjection"),
    (4, "getDynamicSensorList"),
    (5, "createSensorDirectConnection"),
    (6, "setOperationParameter"),
    (7, "getRuntimeSensorList"),
    (8, "enableReplayDataInjection"),
    (9, "enableHalBypassReplayDataInjection"),
    // Present in this image beyond AOSP main's enum: takes an ISensorClientListener, returns a status.
    (10, "registerClientListener"),
];

impl Service for SensorService {
    const DESCRIPTOR: &'static str = "android.gui.SensorServer";
    const TABLE: &'static [(u32, &'static str)] = ISENSORSERVER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getSensorList" | "getDynamicSensorList" | "getRuntimeSensorList" => {
                let pkg: Option<String> = data.read().unwrap_or(None);
                log::info!("sensor: {name} for {pkg:?} -> none");
                reply.write_u32(0)?; // Vector<Sensor> count
                Ok(true)
            }
            "registerClientListener" => {
                let _listener: Option<SIBinder> = data.read().unwrap_or(None);
                reply.write_i32(0)?; // status OK
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
