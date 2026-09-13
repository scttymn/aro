//! Android location callbacks through the consent-gated desktop location portal.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
pub struct LocationService {
    pub host_uid: u32,
    pub setup: Option<Arc<crate::location_setup::HostSetup>>,
    pub authority: SIBinder,
}
struct Cancellation(Arc<AtomicBool>);
impl Service for Cancellation {
    const DESCRIPTOR: &'static str = "android.os.ICancellationSignal";
    const TABLE: &'static [(u32, &'static str)] = &[(1, "cancel")];
    fn handle(&self, _: &str, _: u32, _: &mut Parcel, _: &mut Parcel) -> Result<bool> {
        self.0.store(true, Ordering::Relaxed);
        Ok(true)
    }
}
fn write_fix(p: &mut Parcel, fix: &crate::portal::LocationFix) -> Result<()> {
    ap::string8(p, Some("network"))?;
    p.write_i32(8)?; // HAS_HORIZONTAL_ACCURACY_MASK
    p.write_i64(fix.time_ms)?;
    p.write_i64(fix.elapsed_ns)?;
    p.write_f64(fix.latitude)?;
    p.write_f64(fix.longitude)?;
    p.write_f32(fix.accuracy)?;
    p.write_i32(-1)?; // extras Bundle
    Ok(())
}
fn request(p: &mut Parcel) -> Result<()> {
    if p.read_i32()? == 0 {
        return Ok(());
    }
    let _: Option<String> = p.read()?;
    p.read_i64()?;
    p.read_i32()?;
    p.read_i64()?;
    p.read_i64()?;
    p.read_i32()?;
    p.read_i64()?;
    p.read_f32()?;
    p.read_i64()?;
    for _ in 0..4 {
        p.read_i32()?;
    }
    if p.read_i32()? != 0 {
        p.read_i32()?;
        let n = p.read_i32()?;
        if n > 1024 {
            return Err(rsbinder::StatusCode::BadValue);
        }
        for _ in 0..n.max(0) {
            p.read_i32()?;
        }
        let n = p.read_i32()?;
        if n > 1024 {
            return Err(rsbinder::StatusCode::BadValue);
        }
        for _ in 0..n.max(0) {
            let _: Option<String> = p.read()?;
        }
        if p.read_i32()? >= 0 {
            return Err(rsbinder::StatusCode::BadValue);
        } // attributed chains not yet supported
    }
    Ok(())
}
impl Service for LocationService {
    const DESCRIPTOR: &'static str = "android.location.ILocationManager";
    const TABLE: &'static [(u32, &'static str)] = super::location_codes::CODES;
    fn handle(
        &self,
        name: &str,
        _: TransactionCode,
        data: &mut Parcel,
        reply: &mut Parcel,
    ) -> Result<bool> {
        match name {
            "getAllProviders" | "getProviders" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                ap::string16(reply, Some("network"))?;
            }
            "getBestProvider" => {
                ap::no_exception(reply)?;
                ap::string16(reply, Some("network"))?;
            }
            "isLocationEnabledForUser" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, crate::portal::location_enabled())?;
            }
            "hasProvider" | "isProviderEnabledForUser" => {
                let provider: Option<String> = data.read()?;
                ap::no_exception(reply)?;
                ap::boolean(
                    reply,
                    provider.as_deref() == Some("network")
                        && (name == "hasProvider" || crate::portal::location_enabled()),
                )?;
            }
            "getLastLocation" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
            } // never expose a fix without this request's consent
            "getCurrentLocation" => {
                let provider: Option<String> = data.read()?;
                request(data)?;
                let callback: Option<SIBinder> = data.read()?;
                let package: Option<String> = data.read()?;
                // The supervisor validates the original Binder PID against the
                // attached app. Only this worker may invoke that authority.
                let caller_allowed = crate::host_services::validate_location_caller(
                    &self.authority,
                    rsbinder::thread_state::get_calling_pid(),
                    package.as_deref(),
                )
                .unwrap_or(false);
                let setup = self.setup.clone();
                let canceled = Arc::new(AtomicBool::new(false));
                ap::no_exception(reply)?;
                reply.write(&Some(super::binder_of(Cancellation(canceled.clone()))))?;
                let uid = self.host_uid;
                std::thread::spawn(move || {
                    let fix = if provider.as_deref() == Some("network") {
                        // First-use approval precedes dependency installation and host
                        // enablement. The desktop portal still gates delivery of a fix.
                        let result = (|| -> anyhow::Result<_> {
                            if !caller_allowed {
                                return Ok(None);
                            };
                            let Some(setup) = setup else {
                                return Ok(None);
                            };
                            if !setup.prepare(package.as_deref().unwrap_or(""), &canceled)? {
                                return Ok(None);
                            }
                            crate::portal::location(uid, canceled.clone())
                        })();
                        match result {
                            Ok(f) => f,
                            Err(e) => {
                                log::warn!("location: request failed: {e}");
                                None
                            }
                        }
                    } else {
                        None
                    };
                    if canceled.load(Ordering::Relaxed) {
                        return;
                    }
                    log::info!("location: package={package:?} fix={}", fix.is_some());
                    if let Some(proxy) = callback.as_ref().and_then(|b| b.as_proxy()) {
                        let result = (|| -> Result<()> {
                            let mut d = proxy.prepare_transact(true)?;
                            match fix {
                                Some(f) => ap::typed(&mut d, |p| write_fix(p, &f))?,
                                None => ap::typed_none(&mut d)?,
                            };
                            proxy.submit_transact(1, &d, rsbinder::FLAG_ONEWAY)?;
                            Ok(())
                        })();
                        if let Err(e) = result {
                            log::warn!("location: callback failed: {e}");
                        }
                    }
                });
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn location_parcel_has_only_present_fields() {
        let fix = crate::portal::LocationFix {
            latitude: 1.25,
            longitude: -2.5,
            accuracy: 25.0,
            time_ms: 123,
            elapsed_ns: 456,
        };
        let mut p = Parcel::new();
        write_fix(&mut p, &fix).unwrap();
        p.set_data_position(0);
        assert_eq!(
            ap::read_string8(&mut p).unwrap().as_deref(),
            Some("network")
        );
        assert_eq!(p.read_i32().unwrap(), 8);
        assert_eq!(p.read_i64().unwrap(), 123);
        assert_eq!(p.read_i64().unwrap(), 456);
        assert_eq!(p.read_f64().unwrap(), 1.25);
        assert_eq!(p.read_f64().unwrap(), -2.5);
        assert_eq!(p.read_f32().unwrap(), 25.0);
        assert_eq!(p.read_i32().unwrap(), -1);
        assert_eq!(p.data_position(), p.data_size());
    }
}
