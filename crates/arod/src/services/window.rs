//! `window`: android.view.IWindowManager (M3 will implement it; for now it logs).
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct WindowService;

impl Service for WindowService {
    const DESCRIPTOR: &'static str = "android.view.IWindowManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IWINDOWMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getCurrentAnimatorScale" => {
                ap::no_exception(reply)?;
                reply.write_f32(1.0)?;
                Ok(true)
            }
            "hasNavigationBar" => {
                let _display = data.read_i32()?;
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
