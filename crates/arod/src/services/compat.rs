//! `platform_compat`: com.android.internal.compat.IPlatformCompat.
//!
//! Evaluates app-compat changes the way the platform does, from the image's
//! `compatconfig/*.xml`: a change is enabled unless it is `disabled`, or it
//! is gated on a target SDK the app does not reach.
use super::registry::Registry;
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct Change {
    pub name: String,
    pub disabled: bool,
    pub logging_only: bool,
    /// enableSinceTargetSdk (== enableAfterTargetSdk + 1 in the older spelling)
    pub enable_since_target_sdk: Option<i32>,
}

pub struct PlatformCompatService {
    pub registry: Arc<Registry>,
    pub changes: HashMap<i64, Change>,
}

impl PlatformCompatService {
    /// Load every `<compat-change .../>` under `<system>/system/etc/compatconfig` and `<system>/apex/*/etc/compatconfig`.
    pub fn load(system: &Path, registry: Arc<Registry>) -> Self {
        let mut changes = HashMap::new();
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        for dir in std::iter::once(system.join("system/etc/compatconfig")).chain(std::fs::read_dir(system.join("apex")).into_iter().flatten().flatten().map(|e| e.path().join("etc/compatconfig"))) {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                files.extend(rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "xml").unwrap_or(false)));
            }
        }
        for f in &files {
            let Ok(text) = std::fs::read_to_string(f) else { continue };
            for elem in text.split("<compat-change").skip(1) {
                let elem = elem.split('>').next().unwrap_or("");
                let attr = |k: &str| -> Option<String> { elem.split(&format!("{k}=\"")).nth(1).and_then(|s| s.split('"').next()).map(|s| s.to_string()) };
                let Some(id) = attr("id").and_then(|v| v.parse::<i64>().ok()) else { continue };
                let mut c = Change { name: attr("name").unwrap_or_default(), disabled: attr("disabled").as_deref() == Some("true"), logging_only: attr("loggingOnly").as_deref() == Some("true"), enable_since_target_sdk: None };
                if let Some(v) = attr("enableSinceTargetSdk").and_then(|v| v.parse::<i32>().ok()) {
                    c.enable_since_target_sdk = Some(v);
                } else if let Some(v) = attr("enableAfterTargetSdk").and_then(|v| v.parse::<i32>().ok()) {
                    c.enable_since_target_sdk = Some(v + 1);
                }
                changes.insert(id, c);
            }
        }
        log::info!("compat: {} changes from {} config files", changes.len(), files.len());
        PlatformCompatService { registry, changes }
    }

    fn is_enabled(&self, id: i64, target_sdk: i32) -> bool {
        match self.changes.get(&id) {
            None => true, // unknown change: platform default is enabled
            Some(c) => {
                if c.disabled {
                    return false;
                }
                match c.enable_since_target_sdk {
                    Some(since) => target_sdk >= since,
                    None => true,
                }
            }
        }
    }

    fn target_sdk_of_caller(&self) -> i32 {
        // One app per session for now; refine per calling uid later.
        self.registry.apps.lock().unwrap().first().map(|a| a.target_sdk).unwrap_or(37)
    }
}

impl Service for PlatformCompatService {
    const DESCRIPTOR: &'static str = "com.android.internal.compat.IPlatformCompat";
    const TABLE: &'static [(u32, &'static str)] = codes::IPLATFORMCOMPAT;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "isChangeEnabled" | "isChangeEnabledByPackageName" | "isChangeEnabledByUid" => {
                let change = data.read_i64()?;
                let target_sdk = self.target_sdk_of_caller();
                let enabled = self.is_enabled(change, target_sdk);
                log::debug!("compat: {name} {change} ({}) target {target_sdk} -> {enabled}", self.changes.get(&change).map(|c| c.name.as_str()).unwrap_or("?"));
                ap::no_exception(reply)?;
                ap::boolean(reply, enabled)?;
                Ok(true)
            }
            "reportChange" | "reportChangeByPackageName" | "reportChangeByUid" => {
                ap::no_exception(reply).ok();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
