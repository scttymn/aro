//! `audio`: android.media.IAudioService. No real audio yet (that's M4); this
//! stub keeps AudioManager alive so UI paths like View.playSoundEffect (called
//! from performClick) don't NPE on a null service. Query methods return quiet
//! defaults; everything else is accepted as a no-op.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct AudioService;

impl Service for AudioService {
    const DESCRIPTOR: &'static str = "android.media.IAudioService";
    const TABLE: &'static [(u32, &'static str)] = super::audio_codes::IAUDIOSERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            // Volume/ringer queries with specific sane values.
            "getStreamMinVolume" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getStreamVolume" | "getLastAudibleStreamVolume" => {
                ap::no_exception(reply)?;
                reply.write_i32(10)?;
                Ok(true)
            }
            "getStreamMaxVolume" => {
                ap::no_exception(reply)?;
                reply.write_i32(15)?;
                Ok(true)
            }
            "getRingerModeExternal" | "getRingerModeInternal" => {
                ap::no_exception(reply)?;
                reply.write_i32(2)?; // RINGER_MODE_NORMAL
                Ok(true)
            }
            "getMode" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // MODE_NORMAL
                Ok(true)
            }
            // Everything else: default by name shape.
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("are") || name.starts_with("has") || name.starts_with("should") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                // load/play/set/register/unregister/etc.: void, header only.
                Ok(true)
            }
        }
    }
}
