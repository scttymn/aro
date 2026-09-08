//! PendingIntents. An app creates one through IActivityManager.getIntentSender*;
//! ARO records the wrapped target and hands back an IIntentSender binder. The
//! same binder later appears inside a Notification (its contentIntent), so when
//! the desktop notification is tapped we can look the target back up and launch
//! it — notification tap-back — through the intent dispatcher.
use crate::services::{binder_of, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub struct Target {
    pub package: Option<String>,
    pub class: Option<String>,
    pub action: Option<String>,
    pub data: Option<String>,
}

/// Shared table of live PendingIntents: their sender binder and target.
#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<(SIBinder, Target)>>,
}

impl Registry {
    pub fn new() -> Arc<Registry> {
        Arc::new(Registry::default())
    }

    /// Mint an IIntentSender for `target` and remember it.
    pub fn create(&self, target: Target) -> SIBinder {
        let binder = binder_of(IntentSender { target: target.clone() });
        self.entries.lock().unwrap().push((binder.clone(), target));
        binder
    }

    /// The target of a sender binder, if we minted it.
    pub fn target_for(&self, binder: &SIBinder) -> Option<Target> {
        self.entries.lock().unwrap().iter().find(|(b, _)| b == binder).map(|(_, t)| t.clone())
    }
}

/// Read one android.content.Intent body far enough to recover its target
/// (component/action/data). Leaves the parcel position after the component.
pub fn read_intent_target(data: &mut Parcel) -> Result<Target> {
    let action = ap::read_string8(data)?;
    let uri_type = data.read_i32()?;
    let data_uri = match uri_type {
        0 => None,
        1 => ap::read_string8(data)?,
        _ => return Ok(Target { action, ..Default::default() }), // other Uri shapes: stop early
    };
    let _type = ap::read_string8(data)?;
    let _ident = ap::read_string8(data)?;
    let _flags = data.read_i32()?;
    let _ext = data.read_i32()?;
    let ipkg = ap::read_string8(data)?;
    let cpkg: Option<String> = data.read()?; // ComponentName: String16 package
    let ccls: Option<String> = if cpkg.is_some() { data.read()? } else { None };
    Ok(Target { package: cpkg.or(ipkg), class: ccls, action, data: data_uri })
}

/// android.content.IIntentSender. The app holds this inside a PendingIntent.
/// We fire from the desktop side (tap-back), so send() is a best-effort ack;
/// the real launch is driven by the notifier via the dispatcher.
pub struct IntentSender {
    pub target: Target,
}

impl Service for IntentSender {
    const DESCRIPTOR: &'static str = "android.content.IIntentSender";
    const TABLE: &'static [(u32, &'static str)] = &[(1, "send")];

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        if name == "send" {
            log::info!("pending-intent: send() for {:?}", self.target);
            ap::no_exception(reply)?;
            reply.write_i32(0)?; // sendResult
        }
        Ok(true)
    }
}
