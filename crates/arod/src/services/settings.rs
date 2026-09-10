//! `settings`: SettingsProvider (IContentProvider) and persistent settings storage.
//!
//! Exposes android.content.IContentProvider to handle Settings.System, Settings.Global,
//! Settings.Secure, and DeviceConfig queries. Persists settings across runs in
//! `Layout.data/system/users/0/settings.json`.

use super::Service;
use crate::aparcel as ap;
use crate::bundle;
use rsbinder::{Parcel, Result, TransactionCode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsFile {
    #[serde(default)]
    pub system: HashMap<String, String>,
    #[serde(default)]
    pub secure: HashMap<String, String>,
    #[serde(default)]
    pub global: HashMap<String, String>,
    #[serde(default)]
    pub config: HashMap<String, String>,
}

impl SettingsFile {
    pub fn with_defaults() -> Self {
        let mut system = HashMap::new();
        // Point default ringtones/alarms directly to installed system sound files.
        system.insert("alarm_alert".into(), "file:///product/media/audio/alarms/Alarm_Beep_01.ogg".into());
        system.insert("notification_sound".into(), "file:///product/media/audio/ui/NFCSuccess.ogg".into());
        system.insert("ringtone".into(), "file:///product/media/audio/ringtones/Orion.ogg".into());
        system.insert("time_12_24".into(), "12".into());
        system.insert("volume_alarm_speaker".into(), "7".into());
        system.insert("volume_music_speaker".into(), "7".into());
        system.insert("volume_ring_speaker".into(), "7".into());
        system.insert("volume_voice_speaker".into(), "7".into());
        system.insert("sound_effects_enabled".into(), "1".into());
        system.insert("haptic_feedback_enabled".into(), "1".into());

        let mut global = HashMap::new();
        global.insert("device_provisioned".into(), "1".into());
        global.insert("mode_ringer".into(), "2".into()); // RINGER_MODE_NORMAL
        global.insert("stay_on_while_plugged_in".into(), "0".into());
        global.insert("animator_duration_scale".into(), "1.0".into());
        global.insert("transition_animation_scale".into(), "1.0".into());
        global.insert("window_animation_scale".into(), "1.0".into());

        let mut secure = HashMap::new();
        secure.insert("user_setup_complete".into(), "1".into());

        let config = HashMap::new();

        Self {
            system,
            secure,
            global,
            config,
        }
    }
}

pub struct SettingsService {
    path: PathBuf,
    data: Mutex<SettingsFile>,
}

impl SettingsService {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("system/users/0/settings.json");
        let defaults = SettingsFile::with_defaults();
        let loaded = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<SettingsFile>(&content) {
                    Ok(mut file) => {
                        // Merge missing defaults into existing file
                        for (k, v) in defaults.system {
                            file.system.entry(k).or_insert(v);
                        }
                        for (k, v) in defaults.global {
                            file.global.entry(k).or_insert(v);
                        }
                        for (k, v) in defaults.secure {
                            file.secure.entry(k).or_insert(v);
                        }
                        file
                    }
                    Err(e) => {
                        log::warn!("settings: failed to parse {}: {e}", path.display());
                        defaults
                    }
                },
                Err(e) => {
                    log::warn!("settings: failed to read {}: {e}", path.display());
                    defaults
                }
            }
        } else {
            defaults
        };

        let svc = Self {
            path,
            data: Mutex::new(loaded),
        };
        svc.persist();
        svc
    }

    pub fn get(&self, table: &str, key: &str) -> Option<String> {
        let guard = self.data.lock().unwrap();
        match table {
            "system" => guard.system.get(key).cloned(),
            "secure" => guard.secure.get(key).cloned(),
            "global" => guard.global.get(key).cloned(),
            "config" => guard.config.get(key).cloned(),
            _ => None,
        }
    }

    pub fn put(&self, table: &str, key: &str, value: &str) {
        {
            let mut guard = self.data.lock().unwrap();
            let map = match table {
                "system" => &mut guard.system,
                "secure" => &mut guard.secure,
                "global" => &mut guard.global,
                "config" => &mut guard.config,
                _ => return,
            };
            map.insert(key.to_string(), value.to_string());
        }
        self.persist();
    }

    fn persist(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let guard = self.data.lock().unwrap();
        if let Ok(json) = serde_json::to_string_pretty(&*guard) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

pub struct SettingsProvider {
    pub service: Arc<SettingsService>,
}

impl Service for SettingsProvider {
    const DESCRIPTOR: &'static str = "android.content.IContentProvider";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "QUERY"),
        (2, "GET_TYPE"),
        (21, "CALL"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "CALL" => {
                // Skip AttributionSource
                let start = data.data_position();
                if let Ok(size) = data.read_i32() {
                    if size > 4 {
                        let _ = data.set_data_position(start + size as usize);
                    }
                }
                let authority: Option<String> = data.read().ok().flatten();
                let method: Option<String> = data.read().ok().flatten();
                let string_arg: Option<String> = data.read().ok().flatten();
                log::debug!("settings: CALL auth={authority:?} method={method:?} arg={string_arg:?}");

                let method = method.as_deref().unwrap_or_default();
                let arg = string_arg.as_deref().unwrap_or_default();

                ap::no_exception(reply)?;

                if let Some(table) = method.strip_prefix("GET_") {
                    let val = self.service.get(table, arg);
                    log::info!("settings: GET_{table} '{arg}' -> {val:?}");
                    bundle::write_string_bundle(reply, &[("value", val.as_deref())])?;
                } else if let Some(table) = method.strip_prefix("PUT_") {
                    let (bytes, _) = data.aro_debug_bytes();
                    let mut val = None;
                    for b in bundle::find_all(&bytes) {
                        if let Some(v) = b.get("value") {
                            val = match v {
                                bundle::Value::Str(s) => Some(s.clone()),
                                bundle::Value::Int(i) => Some(i.to_string()),
                                bundle::Value::Long(l) => Some(l.to_string()),
                                bundle::Value::Bool(b) => Some(if *b { "1".to_string() } else { "0".to_string() }),
                                bundle::Value::Float(f) => Some(f.to_string()),
                                bundle::Value::Double(d) => Some(d.to_string()),
                                _ => None,
                            };
                            if val.is_some() {
                                break;
                            }
                        }
                    }
                    if let Some(ref v) = val {
                        log::info!("settings: PUT_{table} '{arg}' = '{v}'");
                        self.service.put(table, arg, v);
                    }
                    bundle::write_string_bundle(reply, &[])?;
                } else {
                    // LIST_config, etc.
                    bundle::write_string_bundle(reply, &[])?;
                }
                Ok(true)
            }
            "GET_TYPE" => {
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
                Ok(true)
            }
            "QUERY" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                Ok(true)
            }
        }
    }
}
