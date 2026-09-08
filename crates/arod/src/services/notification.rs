//! `notification`: android.app.INotificationManager. Android notifications
//! become host desktop notifications (see `crate::notify`). Channels are
//! accepted and remembered only as far as apps need them to exist.
use super::Service;
use crate::aparcel as ap;
use crate::bundle;
use crate::notify::Notifier;
use rsbinder::{Parcel, Result, TransactionCode};
use std::sync::Arc;

pub struct NotificationService {
    pub notifier: Option<Arc<Notifier>>,
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
                log::info!("notification: {pkg} tag={tag:?} id={id}: {summary:?} / {body:?}");
                if let Some(n) = &self.notifier {
                    n.notify(&pkg, tag.as_deref(), id, &pkg, &summary, &body, 1, resident);
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "cancelNotificationWithTag" => {
                let pkg = read_string16(data)?.unwrap_or_default();
                let _op_pkg = read_string16(data)?;
                let tag = read_string16(data)?;
                let id = data.read_i32()?;
                if let Some(n) = &self.notifier { n.close(&pkg, tag.as_deref(), id); }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "cancelAllNotifications" => {
                let pkg = read_string16(data)?.unwrap_or_default();
                if let Some(n) = &self.notifier { n.close_all(&pkg); }
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
