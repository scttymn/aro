//! Host notifications. Whatever owns `org.freedesktop.Notifications` on the
//! session bus (Omarchy's shell today) shows them; ARO never names the daemon.
//! arod keeps its real uid inside the session namespace (see session.rs), so
//! the bus accepts this connection by peer credentials like any other client.
use std::collections::HashMap;
use std::sync::Mutex;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

pub struct Notifier {
    proxy: Proxy<'static>,
    /// (package, tag, id) -> host notification id, so updates replace and cancels close.
    ids: Mutex<HashMap<(String, Option<String>, i32), u32>>,
}

impl Notifier {
    /// `host_uid` is our real uid on the host; the session bus authenticates
    /// EXTERNAL by peer credentials, and inside the namespace geteuid() is 0,
    /// so we assert the real uid explicitly.
    pub fn connect(host_uid: u32) -> anyhow::Result<Notifier> {
        let conn = zbus::blocking::connection::Builder::session()?.user_id(host_uid).build()?;
        let proxy = Proxy::new(&conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications")?;
        if let Ok((name, vendor, version, spec)) = proxy.call::<_, _, (String, String, String, String)>("GetServerInformation", &()) {
            log::info!("notify: host server {name} ({vendor} {version}, spec {spec})");
        }
        Ok(Notifier { proxy, ids: Mutex::new(HashMap::new()) })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify(&self, package: &str, tag: Option<&str>, id: i32, app_name: &str, summary: &str, body: &str, urgency: u8, resident: bool) {
        let key = (package.to_string(), tag.map(str::to_string), id);
        let replaces = self.ids.lock().unwrap().get(&key).copied().unwrap_or(0);
        let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
        hints.insert("urgency", Value::U8(urgency));
        hints.insert("desktop-entry", Value::from(package));
        if resident {
            hints.insert("resident", Value::Bool(true));
        }
        let actions: Vec<&str> = Vec::new();
        let timeout: i32 = if resident { 0 } else { -1 };
        match self.proxy.call::<_, _, u32>("Notify", &(app_name, replaces, "", summary, body, actions, hints, timeout)) {
            Ok(hid) => {
                self.ids.lock().unwrap().insert(key, hid);
            }
            Err(e) => log::warn!("notify: Notify failed: {e}"),
        }
    }

    pub fn close(&self, package: &str, tag: Option<&str>, id: i32) {
        let key = (package.to_string(), tag.map(str::to_string), id);
        if let Some(host_id) = self.ids.lock().unwrap().remove(&key) {
            let _: Result<(), _> = self.proxy.call("CloseNotification", &(host_id,));
        }
    }

    pub fn close_all(&self, package: &str) {
        let mut ids = self.ids.lock().unwrap();
        let keys: Vec<_> = ids.keys().filter(|k| k.0 == package).cloned().collect();
        for k in keys {
            if let Some(host_id) = ids.remove(&k) {
                let _: Result<(), _> = self.proxy.call("CloseNotification", &(host_id,));
            }
        }
    }
}
