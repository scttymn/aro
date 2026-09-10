//! `shortcut`: android.content.pm.IShortcutService.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct ShortcutService;

pub static ISHORTCUTSERVICE: &[(u32, &str)] = &[
    (1, "setDynamicShortcuts"),
    (2, "addDynamicShortcuts"),
    (3, "removeDynamicShortcuts"),
    (4, "removeAllDynamicShortcuts"),
    (5, "updateShortcuts"),
    (6, "requestPinShortcut"),
    (7, "createShortcutResultIntent"),
    (8, "disableShortcuts"),
    (9, "enableShortcuts"),
    (10, "getMaxShortcutCountPerActivity"),
    (11, "getRemainingCallCount"),
    (12, "getRateLimitResetTime"),
    (13, "getIconMaxDimensions"),
    (14, "reportShortcutUsed"),
    (15, "resetThrottling"),
    (16, "onApplicationActive"),
    (17, "getBackupPayload"),
    (18, "applyRestore"),
    (19, "isRequestPinItemSupported"),
    (20, "getShareTargets"),
    (21, "hasShareTargets"),
    (22, "removeLongLivedShortcuts"),
    (23, "getShortcuts"),
    (24, "pushDynamicShortcut"),
];

impl Service for ShortcutService {
    const DESCRIPTOR: &'static str = "android.content.pm.IShortcutService";
    const TABLE: &'static [(u32, &'static str)] = ISHORTCUTSERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getMaxShortcutCountPerActivity" => {
                ap::no_exception(reply)?;
                reply.write_i32(15)?;
                Ok(true)
            }
            "getRemainingCallCount" => {
                ap::no_exception(reply)?;
                reply.write_i32(10)?;
                Ok(true)
            }
            "getRateLimitResetTime" => {
                ap::no_exception(reply)?;
                reply.write_i64(0)?;
                Ok(true)
            }
            "getIconMaxDimensions" => {
                ap::no_exception(reply)?;
                reply.write_i32(128)?;
                Ok(true)
            }
            "isRequestPinItemSupported" | "hasShareTargets" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "setDynamicShortcuts"
            | "addDynamicShortcuts"
            | "updateShortcuts"
            | "removeDynamicShortcuts"
            | "removeAllDynamicShortcuts"
            | "removeLongLivedShortcuts"
            | "disableShortcuts"
            | "enableShortcuts" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "getShortcuts" | "getShareTargets" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "reportShortcutUsed" | "resetThrottling" | "onApplicationActive" | "pushDynamicShortcut" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("has") || name.starts_with("can") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}
