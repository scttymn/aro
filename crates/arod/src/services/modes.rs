//! `contextual_mode`: android.app.modes.IContextualModeManager.
//! Manages contextual modes, zen (Do Not Disturb) mode, and notification policies.

use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct ModesService;

const TABLE: &'static [(u32, &'static str)] = &[
    (1, "isModeSyncSupported"),
    (2, "isModeSyncEnabled"),
    (3, "setModeSyncEnabled"),
    (4, "getModes"),
    (5, "mutateModes"),
    (6, "registerModeSyncListener"),
    (7, "unregisterModeSyncListener"),
    (8, "registerModeListener"),
    (9, "unregisterModeListener"),
    (10, "getZenMode"),
    (11, "getZenModeConfig"),
    (12, "getConsolidatedNotificationPolicy"),
    (13, "setZenMode"),
    (14, "notifyConditions"),
    (15, "getNotificationPolicy"),
    (16, "setNotificationPolicy"),
    (17, "getDefaultZenPolicy"),
    (18, "getAutomaticZenRule"),
    (19, "getAutomaticZenRules"),
    (20, "addAutomaticZenRule"),
    (21, "updateAutomaticZenRule"),
    (22, "removeAutomaticZenRule"),
    (23, "removeAutomaticZenRules"),
    (24, "getRuleInstanceCount"),
    (25, "getAutomaticZenRuleState"),
    (26, "setAutomaticZenRuleState"),
    (27, "setManualZenRuleDeviceEffects"),
    (28, "setInterruptionFilter"),
    (29, "matchesCallFilter"),
    (30, "cleanUpCallersAfter"),
    (31, "requestBindProvider"),
    (32, "requestUnbindProvider"),
    (33, "requestInterruptionFilterFromListener"),
    (34, "isNotificationPolicyAccessGranted"),
    (35, "isNotificationPolicyAccessGrantedForPackage"),
    (36, "setNotificationPolicyAccessGranted"),
    (37, "setNotificationPolicyAccessGrantedForUser"),
    (38, "getEnabledZenPackages"),
];

impl Service for ModesService {
    const DESCRIPTOR: &'static str = "android.app.modes.IContextualModeManager";
    const TABLE: &'static [(u32, &'static str)] = TABLE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getZenMode" => {
                // 0 = ZEN_MODE_OFF
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "isModeSyncSupported" | "isModeSyncEnabled" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "isNotificationPolicyAccessGranted" | "isNotificationPolicyAccessGrantedForPackage" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "getAutomaticZenRules" => {
                // ParceledListSlice, empty
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getModes" | "getEnabledZenPackages" => {
                // Empty List: size 0
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getConsolidatedNotificationPolicy"
            | "getNotificationPolicy"
            | "getDefaultZenPolicy"
            | "getAutomaticZenRule"
            | "getZenModeConfig" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "getRuleInstanceCount" | "getAutomaticZenRuleState" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "registerModeListener"
            | "unregisterModeListener"
            | "registerModeSyncListener"
            | "unregisterModeSyncListener"
            | "setModeSyncEnabled"
            | "setZenMode"
            | "notifyConditions"
            | "setNotificationPolicy"
            | "setAutomaticZenRuleState"
            | "setManualZenRuleDeviceEffects"
            | "setInterruptionFilter"
            | "cleanUpCallersAfter"
            | "requestBindProvider"
            | "requestUnbindProvider"
            | "requestInterruptionFilterFromListener"
            | "setNotificationPolicyAccessGranted"
            | "setNotificationPolicyAccessGrantedForUser" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("are") || name.starts_with("can") || name.starts_with("matches") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
        }
    }
}
