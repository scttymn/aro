//! `media.camera`: android.hardware.ICameraService. No cameras are exposed
//! yet; the desktop camera can be bridged later (subsystem: media).
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};

pub struct CameraService;

impl Service for CameraService {
    const DESCRIPTOR: &'static str = "android.hardware.ICameraService";
    const TABLE: &'static [(u32, &'static str)] = codes::ICAMERASERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getNumberOfCameras" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "addListener" => {
                let _listener: Option<SIBinder> = data.read()?;
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // CameraStatus[]: empty
                Ok(true)
            }
            "removeListener" | "notifyDisplayConfigurationChange" | "notifyDeviceStateChange" | "notifySystemEvent" => {
                ap::no_exception(reply).ok();
                Ok(true)
            }
            "getConcurrentCameraIds" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // ConcurrentCameraIdCombination[]: empty
                Ok(true)
            }
            "isHiddenPhysicalCamera" | "isConcurrentSessionConfigurationSupported" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
