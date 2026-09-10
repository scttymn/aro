//! `notification`: android.app.INotificationManager. Android notifications
//! become host desktop notifications (see `crate::notify`). Channels are
//! accepted and remembered only as far as apps need them to exist.
use super::Service;
use crate::aparcel as ap;
use crate::bundle;
use crate::pending_intent::{Registry as PiRegistry, Target};
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::Arc;

pub struct NotificationService {
    pub pending_intents: Arc<PiRegistry>,
    pub shade: Arc<crate::shade::Shade>,
    /// This app's launcher icon path for the shade avatar, or None for a badge.
    pub app_icon: Option<String>,
}

// flat_binder_object header type B_PACK_CHARS('s','b','*',0x85): a binder we published,
// routed back to us as the loopback publisher (contentIntent's IIntentSender).
const BINDER_TYPE_BINDER: i32 = 0x7362_2a85u32 as i32;

/// Scan the Notification parcel's binder objects for one that matches a
/// PendingIntent we minted, and return its launch target. The contentIntent's
/// IIntentSender rides in the parcel as a binder object; reading it back gives
/// the same SIBinder we handed out from getIntentSender.
fn tap_target(data: &mut Parcel, reg: &PiRegistry) -> Option<Target> {
    let (bytes, objs) = data.aro_debug_bytes();
    let rd = |o: usize| -> i32 {
        i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]])
    };
    let save = data.data_position();
    let mut found = None;
    for &o in &objs {
        let o = o as usize;
        if o + 4 > bytes.len() || rd(o) != BINDER_TYPE_BINDER {
            continue;
        }
        data.set_data_position(o);
        if let Ok(Some(b)) = data.read::<Option<SIBinder>>() {
            if let Some(t) = reg.target_for(&b) {
                found = Some(t);
                break;
            }
        }
    }
    data.set_data_position(save);
    found
}

// Notification.FLAG_ONGOING_EVENT
const FLAG_ONGOING_EVENT: i32 = 0x0000_0002;

fn read_string16(data: &mut Parcel) -> Result<Option<String>> {
    let s: Option<String> = data.read()?;
    Ok(s)
}

impl Service for NotificationService {
    const DESCRIPTOR: &'static str = "android.app.INotificationManager";
    const TABLE: &'static [(u32, &'static str)] = super::notification_codes::INOTIFICATIONMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "enqueueNotificationWithTag" => {
                // (String pkg, String opPkg, String tag, int id, in Notification, int userId)
                let pkg = read_string16(data)?.unwrap_or_default();
                let _op_pkg = read_string16(data)?;
                let tag = read_string16(data)?;
                let id = data.read_i32()?;
                // The Notification itself is deep (icons, pending intents, remote views);
                // lift what the desktop needs out of its extras bundle.
                let (bytes, _) = data.aro_debug_bytes();
                let mut title = None;
                let mut text = None;
                let mut sub = None;
                for b in bundle::find_all(&bytes) {
                    if title.is_none() { title = b.get("android.title").and_then(|v| v.as_str()).map(str::to_string); }
                    if text.is_none() {
                        text = b.get("android.bigText").or_else(|| b.get("android.text")).and_then(|v| v.as_str()).map(str::to_string);
                    }
                    if sub.is_none() { sub = b.get("android.subText").and_then(|v| v.as_str()).map(str::to_string); }
                    if title.is_some() && text.is_some() { break; }
                }
                // Ongoing flag: Notification.flags is written early (after the icons); not
                // walked here, so treat everything as transient for now.
                let resident = false;
                let _ = FLAG_ONGOING_EVENT;
                let summary = title.clone().unwrap_or_else(|| pkg.clone());
                let body = match (text, sub) {
                    (Some(t), Some(s)) => format!("{t}\n{s}"),
                    (Some(t), None) => t,
                    (None, Some(s)) => s,
                    (None, None) => String::new(),
                };
                let target = tap_target(data, &self.pending_intents);
                log::info!("notification: {pkg} tag={tag:?} id={id}: {summary:?} / {body:?} tap={target:?}");
                // The shade keeps the notification after the toast fades. ongoing
                // detection (Notification.flags) is not wired yet — see shade.rs.
                let key = (pkg.clone(), tag.clone(), id);
                self.shade.post(key, &pkg, &pkg, &summary, &body, resident, self.app_icon.as_deref().unwrap_or(""), target);
                ap::no_exception(reply)?;
                Ok(true)
            }
            "cancelNotificationWithTag" => {
                let pkg = read_string16(data)?.unwrap_or_default();
                let _op_pkg = read_string16(data)?;
                let tag = read_string16(data)?;
                let id = data.read_i32()?;
                self.shade.remove(&(pkg.clone(), tag.clone(), id));
                ap::no_exception(reply)?;
                Ok(true)
            }
            "cancelAllNotifications" => {
                let pkg = read_string16(data)?.unwrap_or_default();
                self.shade.remove_all(&pkg);
                ap::no_exception(reply)?;
                Ok(true)
            }
            "areNotificationsEnabled" | "areNotificationsEnabledForPackage" | "canShowBadge" | "canNotifyAsPackage" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "getImportance" | "getPackageImportance" => {
                ap::no_exception(reply)?;
                reply.write_i32(3)?; // IMPORTANCE_DEFAULT
                Ok(true)
            }
            "getZenMode" | "getInterruptionFilterFromListener" | "getBubblePreferenceForPackage" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getNotificationChannels" | "getNotificationChannelGroups" | "getActiveNotifications" | "getAppActiveNotifications" | "getNotificationChannelsForPackage" => {
                // ParceledListSlice, empty
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("are") || name.starts_with("can") || name.starts_with("should") || name.starts_with("has") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
        }
    }
}
