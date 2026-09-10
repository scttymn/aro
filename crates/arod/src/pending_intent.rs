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
pub struct RawBundle {
    pub length: i32,
    pub magic: i32,
    pub data: Vec<u8>,
    pub has_intent: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Target {
    pub intent_type: i32,
    pub package: Option<String>,
    pub class: Option<String>,
    pub action: Option<String>,
    pub data: Option<String>,
    pub categories: Vec<String>,
    pub extras: Option<RawBundle>,
}

impl Target {
    pub fn to_intent(&self) -> crate::parcelables::Intent {
        crate::parcelables::Intent {
            action: self.action.clone(),
            data: self.data.clone(),
            package: self.package.clone(),
            component: match (&self.package, &self.class) {
                (Some(p), Some(c)) => Some((p.clone(), c.clone())),
                _ => None,
            },
            categories: self.categories.clone(),
            flags: 0,
            extras: self.extras.clone(),
        }
    }
}

/// Shared table of live PendingIntents: their sender binder and target.
#[derive(Default)]
pub struct Registry {
    entries: Mutex<Vec<(SIBinder, Target)>>,
    fire: Mutex<Option<Arc<dyn Fn(&Target) + Send + Sync>>>,
}

impl Registry {
    pub fn new() -> Arc<Registry> {
        Arc::new(Registry::default())
    }

    pub fn set_fire_handler(&self, fire: Arc<dyn Fn(&Target) + Send + Sync>) {
        *self.fire.lock().unwrap() = Some(fire);
    }

    /// Mint an IIntentSender for `target` and remember it.
    pub fn create(&self, target: Target) -> SIBinder {
        let fire = self.fire.lock().unwrap().clone();
        let binder = binder_of(IntentSender { target: target.clone(), fire });
        self.entries.lock().unwrap().push((binder.clone(), target));
        binder
    }

    /// The target of a sender binder, if we minted it.
    pub fn target_for(&self, binder: &SIBinder) -> Option<Target> {
        self.entries.lock().unwrap().iter().find(|(b, _)| b == binder).map(|(_, t)| t.clone())
    }
}

/// Read one android.content.Intent body far enough to recover its target
/// (component/action/data/extras). Leaves the parcel position after the component/extras.
pub fn read_intent_target(data: &mut Parcel) -> Result<Target> {
    let action = ap::read_string8(data)?;
    let uri_type = data.read_i32()?;
    let data_uri = match uri_type {
        0 => None,
        1 | 2 | 3 => ap::read_string8(data)?,
        _ => return Ok(Target { action, ..Default::default() }), // unknown Uri shapes: stop early
    };
    let _type = ap::read_string8(data)?;
    let _ident = ap::read_string8(data)?;
    let _flags = data.read_i32()?;
    let _ext = data.read_i32()?;
    let ipkg = ap::read_string8(data)?;
    let cpkg: Option<String> = data.read()?; // ComponentName: String16 package
    let ccls: Option<String> = if cpkg.is_some() { data.read()? } else { None };

    // Source bounds (Rect)
    let has_source_bounds = data.read_i32()?;
    if has_source_bounds != 0 {
        data.read_i32()?; data.read_i32()?; data.read_i32()?; data.read_i32()?;
    }

    // Categories
    let cat_count = data.read_i32()?;
    let mut categories = Vec::new();
    if cat_count > 0 {
        for _ in 0..cat_count {
            if let Some(c) = ap::read_string8(data)? {
                categories.push(c);
            }
        }
    }

    // Selector
    let has_selector = data.read_i32()?;
    if has_selector != 0 {
        log::warn!("read_intent_target: selector present, stopping parse early");
        return Ok(Target { package: cpkg.or(ipkg), class: ccls, action, data: data_uri, categories, ..Default::default() });
    }

    // ClipData
    let has_clipdata = data.read_i32()?;
    if has_clipdata != 0 {
        log::warn!("read_intent_target: clipdata present, stopping parse early");
        return Ok(Target { package: cpkg.or(ipkg), class: ccls, action, data: data_uri, categories, ..Default::default() });
    }

    // ContentUserHint
    let _content_user_hint = data.read_i32()?;

    // Extras (writeBundle)
    let bundle_len = data.read_i32()?;
    let extras = if bundle_len > 0 {
        let magic = data.read_i32()?;
        let total = (bundle_len as usize + 3) & !3;
        let mut bytes = Vec::with_capacity(total);
        for _ in 0..total / 4 {
            bytes.extend_from_slice(&data.read_u32()?.to_le_bytes());
        }
        let has_intent = data.read_i32()?;
        Some(RawBundle {
            length: bundle_len,
            magic,
            data: bytes,
            has_intent,
        })
    } else if bundle_len == 0 {
        Some(RawBundle {
            length: 0,
            magic: 0,
            data: Vec::new(),
            has_intent: 0,
        })
    } else {
        None
    };

    if let Some(ref b) = extras {
        if b.length > 0 {
            let mut buf = Vec::with_capacity(4 + b.data.len());
            buf.extend_from_slice(&b.magic.to_le_bytes());
            buf.extend_from_slice(&b.data);
            let parsed = crate::bundle::parse_at(&buf, 0);
            log::info!("read_intent_target: extras={parsed:?}");
        }
    }

    // OriginalIntent and CreatorTokenInfo if present
    let has_orig = data.read_i32().unwrap_or(0);
    if has_orig != 0 {
        log::warn!("read_intent_target: original intent present");
    }
    let _creator_token = data.read_i32().unwrap_or(0);

    let pkg = cpkg.or(ipkg);
    log::info!("read_intent_target: action={action:?} uri={data_uri:?} pkg={pkg:?} cls={ccls:?}");
    Ok(Target {
        intent_type: 0,
        package: pkg,
        class: ccls,
        action,
        data: data_uri,
        categories,
        extras,
    })
}

/// android.content.IIntentSender. The app holds this inside a PendingIntent.
pub struct IntentSender {
    pub target: Target,
    pub fire: Option<Arc<dyn Fn(&Target) + Send + Sync>>,
}

impl Service for IntentSender {
    const DESCRIPTOR: &'static str = "android.content.IIntentSender";
    const TABLE: &'static [(u32, &'static str)] = &[(1, "send")];

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        if name == "send" {
            log::info!("pending-intent: send() for {:?}", self.target);
            if let Some(ref fire) = self.fire {
                fire(&self.target);
            }
            ap::no_exception(reply)?;
            reply.write_i32(0)?; // sendResult
        }
        Ok(true)
    }
}
