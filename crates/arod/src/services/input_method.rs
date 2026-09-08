//! `input_method`: com.android.internal.view.IInputMethodManager. No IME yet;
//! clients register and get nothing back, which the framework tolerates.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct InputMethodService;

impl Service for InputMethodService {
    const DESCRIPTOR: &'static str = "com.android.internal.view.IInputMethodManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IINPUTMETHODMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "addClient" | "startInputOrWindowGainedFocus" | "reportPerceptible" | "removeImeSurfaceFromWindow" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "isImeTraceEnabled" | "isStylusHandwritingAvailableAsUser" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getInputMethodList" | "getEnabledInputMethodList" | "getEnabledInputMethodSubtypeList" => {
                ap::no_exception(reply)?;
                ap::null_array(reply)?; // List<InputMethodInfo>: null (Java treats as empty)
                Ok(true)
            }
            "getCurrentInputMethodInfoAsUser" | "getLastInputMethodSubtype" | "getCurrentInputMethodSubtype" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
