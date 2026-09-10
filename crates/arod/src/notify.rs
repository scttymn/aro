//! Host notifications. Whatever owns `org.freedesktop.Notifications` on the
//! session bus (Omarchy's shell today) shows them; ARO never names the daemon.
//! arod keeps its real uid inside the session namespace (see session.rs), so
//! the bus accepts this connection by peer credentials like any other client.
//!
//! Tapping a host notification fires the Android contentIntent: each posted
//! notification carries a "default" action, and a background thread listens for
//! the daemon's `ActionInvoked` signal to re-launch the PendingIntent target
//! (see `crate::pending_intent`).
use crate::pending_intent::Target;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

/// Called with a tapped notification's launch target.
pub type Fire = Arc<dyn Fn(&Target) + Send + Sync>;

struct ActiveNote {
    host_id: u32,
    last_summary: String,
    _last_posted: Instant,
}

pub struct Notifier {
    proxy: Proxy<'static>,
    /// (package, tag, id) -> active host notification, so updates replace and cancels close.
    active: Mutex<HashMap<(String, Option<String>, i32), ActiveNote>>,
    /// host notification id -> launch target, so ActionInvoked can re-open the app.
    targets: Arc<Mutex<HashMap<u32, Target>>>,
}

impl Notifier {
    /// `host_uid` is our real uid on the host; the session bus authenticates
    /// EXTERNAL by peer credentials, and inside the namespace geteuid() is 0,
    /// so we assert the real uid explicitly. `fire` re-launches a tapped
    /// notification's PendingIntent target.
    pub fn connect(host_uid: u32, fire: Fire) -> anyhow::Result<Notifier> {
        let conn = zbus::blocking::connection::Builder::session()?.user_id(host_uid).build()?;
        let proxy = Proxy::new(&conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications")?;
        if let Ok((name, vendor, version, spec)) = proxy.call::<_, _, (String, String, String, String)>("GetServerInformation", &()) {
            log::info!("notify: host server {name} ({vendor} {version}, spec {spec})");
        }
        let targets = Arc::new(Mutex::new(HashMap::new()));
        Self::spawn_listener(conn, targets.clone(), fire);
        Ok(Notifier { proxy, active: Mutex::new(HashMap::new()), targets })
    }

    /// Watch `ActionInvoked` (tap) and `NotificationClosed` (dismiss) so a tap
    /// re-launches the target and a close forgets it. Both are handled on a
    /// single stream in bus order: a client that invokes an action then
    /// dismisses (Omarchy's shell does exactly this) emits ActionInvoked before
    /// NotificationClosed, so reading them in order fires before we forget.
    fn spawn_listener(conn: Connection, targets: Arc<Mutex<HashMap<u32, Target>>>, fire: Fire) {
        std::thread::spawn(move || {
            let proxy = match Proxy::new(&conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications") {
                Ok(p) => p,
                Err(e) => { log::warn!("notify: listener proxy: {e}"); return; }
            };
            let signals = match proxy.receive_all_signals() {
                Ok(s) => s,
                Err(e) => { log::warn!("notify: subscribe signals: {e}"); return; }
            };
            for msg in signals {
                let member = msg.header().member().map(|m| m.to_string());
                match member.as_deref() {
                    Some("ActionInvoked") => {
                        if let Ok((hid, action)) = msg.body().deserialize::<(u32, String)>() {
                            log::info!("notify: ActionInvoked host id {hid} action {action:?}");
                            let target = targets.lock().unwrap().get(&hid).cloned();
                            if let Some(t) = target {
                                fire(&t);
                            }
                        }
                    }
                    Some("NotificationClosed") => {
                        if let Ok((hid, _reason)) = msg.body().deserialize::<(u32, u32)>() {
                            targets.lock().unwrap().remove(&hid);
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify(&self, package: &str, tag: Option<&str>, id: i32, app_name: &str, summary: &str, body: &str, urgency: u8, resident: bool, target: Option<Target>) {
        let key = (package.to_string(), tag.map(str::to_string), id);
        let mut active = self.active.lock().unwrap();
        let replaces = if let Some(entry) = active.get(&key) {
            // Suppress repetitive host toasts if the notification was already posted and
            // either it is resident/ongoing or the summary has not changed (e.g. 1Hz distance updates).
            if resident || entry.last_summary == summary {
                log::debug!("notify: suppressing repetitive host toast for {package} id={id} (same summary/resident)");
                return;
            }
            entry.host_id
        } else {
            0
        };

        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("urgency", Value::U8(urgency));
        hints.insert("desktop-entry", Value::from(package));
        if resident {
            hints.insert("resident", Value::Bool(true));
        }
        // A "default" action makes the notification body itself clickable; the
        // daemon reports it back via ActionInvoked when the user taps.
        let has_target = target.is_some();
        let actions: Vec<&str> = if has_target { vec!["default", "Open"] } else { Vec::new() };
        // Normal toasts expire after desktop default (-1); resident ones persist (0).
        let timeout: i32 = if resident { 0 } else { -1 };
        match self.proxy.call::<_, _, u32>("Notify", &(app_name, replaces, "", summary, body, actions, hints, timeout)) {
            Ok(hid) => {
                log::debug!("notify: posted host id {hid} (replaces={replaces}, target={has_target})");
                active.insert(key, ActiveNote {
                    host_id: hid,
                    last_summary: summary.to_string(),
                    _last_posted: Instant::now(),
                });
                match target {
                    Some(t) => { self.targets.lock().unwrap().insert(hid, t); }
                    None => { self.targets.lock().unwrap().remove(&hid); }
                }
            }
            Err(e) => log::warn!("notify: Notify failed: {e}"),
        }
    }

    pub fn close(&self, package: &str, tag: Option<&str>, id: i32) {
        let key = (package.to_string(), tag.map(str::to_string), id);
        if let Some(entry) = self.active.lock().unwrap().remove(&key) {
            self.targets.lock().unwrap().remove(&entry.host_id);
            let _: Result<(), _> = self.proxy.call("CloseNotification", &(entry.host_id,));
        }
    }

    pub fn close_all(&self, package: &str) {
        let mut active = self.active.lock().unwrap();
        let keys: Vec<_> = active.keys().filter(|k| k.0 == package).cloned().collect();
        for k in keys {
            if let Some(entry) = active.remove(&k) {
                self.targets.lock().unwrap().remove(&entry.host_id);
                let _: Result<(), _> = self.proxy.call("CloseNotification", &(entry.host_id,));
            }
        }
    }
}
