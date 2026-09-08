//! Host-side system services on the bus.
//!
//! Each service is a raw Binder object: it receives transactions by code and
//! answers them by hand-written parcel I/O, using the transaction tables and
//! parcel layouts extracted from the image (`spec/`). Anything unimplemented
//! is logged by name and rejected, so a run of a real app prints the exact
//! list of calls still needed.
pub mod accessibility;
pub mod activity;
pub mod activity_task;
pub mod allocator;
pub mod audio;
pub mod audio_codes;
pub mod camera;
pub mod compat;
pub mod display;
pub mod input;
pub mod input_codes;
pub mod input_method;
pub mod network;
pub mod network_codes;
pub mod notification;
pub mod notification_codes;
pub mod package;
pub mod registry;
pub mod sensor;
pub mod surfaceflinger;
pub mod token;
pub mod user;
pub mod window;
pub mod window_session;

use rsbinder::{Parcel, Remotable, Result, StatusCode, TransactionCode};

#[allow(dead_code)]
pub mod codes {
    include!("../../../../spec/transactions.rs");
}

pub fn name_of(table: &[(u32, &'static str)], code: u32) -> &'static str {
    table.iter().find(|(c, _)| *c == code).map(|(_, n)| *n).unwrap_or("?")
}

/// Shared boilerplate: every service handles `code` by name and returns
/// `Ok(true)` if handled, `Ok(false)` if not implemented.
pub trait Service: Send + Sync + 'static {
    const DESCRIPTOR: &'static str;
    const TABLE: &'static [(u32, &'static str)];
    fn handle(&self, name: &str, code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool>;
}

/// Adapts a `Service` into an rsbinder `Remotable`.
pub struct Raw<S: Service>(pub S);

impl<S: Service> Remotable for Raw<S> {
    fn descriptor() -> &'static str {
        S::DESCRIPTOR
    }

    fn on_transact(&self, code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<()> {
        let name = name_of(S::TABLE, code);
        let pid = rsbinder::thread_state::get_calling_pid();
        match self.0.handle(name, code, data, reply) {
            Ok(true) => {
                log::debug!("{}.{name} from pid {pid}: ok", S::DESCRIPTOR);
                Ok(())
            }
            Ok(false) => {
                log::warn!("{}.{name} (code {code}) from pid {pid}: NOT IMPLEMENTED", S::DESCRIPTOR);
                Err(StatusCode::UnknownTransaction)
            }
            Err(e) => {
                log::error!("{}.{name} from pid {pid}: {e:?}", S::DESCRIPTOR);
                Err(e)
            }
        }
    }

    fn on_dump(&self, _writer: &mut dyn std::io::Write, _args: &[String]) -> Result<()> {
        Ok(())
    }
}

/// Wrap a service into a binder object without publishing it by name.
pub fn binder_of<S: Service>(service: S) -> rsbinder::SIBinder {
    let binder = rsbinder::native::Binder::new(Raw(service));
    rsbinder::Interface::as_binder(&binder)
}

/// A binder with @VintfStability (HAL interfaces such as the gralloc allocator).
pub fn vintf_binder_of<S: Service>(service: S) -> rsbinder::SIBinder {
    let binder = rsbinder::native::Binder::new_with_stability(Raw(service), rsbinder::Stability::Vintf);
    rsbinder::Interface::as_binder(&binder)
}

/// Register a service with the hub under `name`.
pub fn publish<S: Service>(hub: &crate::hub::Hub, name: &str, service: S) -> rsbinder::SIBinder {
    let sibinder = binder_of(service);
    hub.register(name, sibinder.clone());
    log::info!("service {name} ({}) published", S::DESCRIPTOR);
    sibinder
}
