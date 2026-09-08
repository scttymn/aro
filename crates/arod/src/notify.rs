//! Host notifications. Whatever owns `org.freedesktop.Notifications` on the
//! session bus (Omarchy's shell today) shows them; ARO never names the daemon.
//!
//! D-Bus authenticates by peer credentials, and arod runs inside a user
//! namespace (uid 0 there), so a connection made from arod is rejected by the
//! bus. We therefore fork a small helper BEFORE the namespace is entered: it
//! stays in the host session with the real uid, owns the D-Bus connection and
//! the Android→host id mapping, and takes framed commands from arod over a
//! socketpair. This is the general seam for host daemons that check peer creds.
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;

const OP_NOTIFY: u8 = 1;
const OP_CLOSE: u8 = 2;
const OP_CLOSE_ALL: u8 = 3;

/// arod-side handle: frames commands to the helper. Cheap to hold behind a lock.
pub struct Notifier {
    sock: Mutex<UnixStream>,
}

fn put_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}
fn put_opt(buf: &mut Vec<u8>, s: Option<&str>) {
    match s {
        Some(s) => {
            buf.push(1);
            put_str(buf, s);
        }
        None => buf.push(0),
    }
}

impl Notifier {
    /// Fork the host-side helper. Call while single-threaded and BEFORE any
    /// namespace unshare. Returns None if there is no session bus.
    pub fn spawn() -> Option<Notifier> {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
            log::warn!("notify: DBUS_SESSION_BUS_ADDRESS unset; notifications logged only");
            return None;
        }
        let (ours, theirs) = UnixStream::pair().ok()?;
        // SAFETY: arod is single-threaded here; the child only runs async-signal-safe
        // work until it has fully replaced its own state (it never returns to main).
        match unsafe { libc::fork() } {
            -1 => {
                log::warn!("notify: fork failed: {}", std::io::Error::last_os_error());
                None
            }
            0 => {
                drop(ours);
                // Die with arod.
                unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) };
                let code = helper_main(theirs);
                std::process::exit(code);
            }
            _pid => {
                drop(theirs);
                Some(Notifier { sock: Mutex::new(ours) })
            }
        }
    }

    fn send(&self, buf: &[u8]) {
        let mut frame = (buf.len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(buf);
        if let Ok(mut s) = self.sock.lock() {
            let _ = s.write_all(&frame);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn notify(&self, package: &str, tag: Option<&str>, id: i32, app_name: &str, summary: &str, body: &str, urgency: u8, resident: bool) {
        let mut b = vec![OP_NOTIFY];
        put_str(&mut b, package);
        put_opt(&mut b, tag);
        b.extend_from_slice(&id.to_le_bytes());
        put_str(&mut b, app_name);
        put_str(&mut b, summary);
        put_str(&mut b, body);
        b.push(urgency);
        b.push(resident as u8);
        self.send(&b);
    }

    pub fn close(&self, package: &str, tag: Option<&str>, id: i32) {
        let mut b = vec![OP_CLOSE];
        put_str(&mut b, package);
        put_opt(&mut b, tag);
        b.extend_from_slice(&id.to_le_bytes());
        self.send(&b);
    }

    pub fn close_all(&self, package: &str) {
        let mut b = vec![OP_CLOSE_ALL];
        put_str(&mut b, package);
        self.send(&b);
    }
}

// ---- helper process (host namespace, real uid) ----

struct Cur<'a>(&'a [u8], usize);
impl<'a> Cur<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.0.get(self.1)?;
        self.1 += 1;
        Some(v)
    }
    fn i32(&mut self) -> Option<i32> {
        let v = self.0.get(self.1..self.1 + 4)?;
        self.1 += 4;
        Some(i32::from_le_bytes(v.try_into().ok()?))
    }
    fn s(&mut self) -> Option<String> {
        let n = self.i32()? as usize;
        let v = self.0.get(self.1..self.1 + n)?;
        self.1 += n;
        Some(String::from_utf8_lossy(v).into_owned())
    }
    fn opt(&mut self) -> Option<Option<String>> {
        match self.u8()? {
            0 => Some(None),
            _ => Some(Some(self.s()?)),
        }
    }
}

fn helper_main(sock: UnixStream) -> i32 {
    use std::collections::HashMap;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::Value;

    let conn = match Connection::session() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("aro-notify: session bus: {e}");
            return 1;
        }
    };
    let proxy = match Proxy::new(&conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aro-notify: proxy: {e}");
            return 1;
        }
    };
    if let Ok((name, vendor, version, spec)) = proxy.call::<_, _, (String, String, String, String)>("GetServerInformation", &()) {
        eprintln!("aro-notify: host server {name} ({vendor} {version}, spec {spec})");
    }

    let mut ids: HashMap<(String, Option<String>, i32), u32> = HashMap::new();
    let mut sock = sock;
    let mut lenbuf = [0u8; 4];
    loop {
        if sock.read_exact(&mut lenbuf).is_err() {
            return 0; // arod closed the pipe / exited
        }
        let len = u32::from_le_bytes(lenbuf) as usize;
        let mut buf = vec![0u8; len];
        if sock.read_exact(&mut buf).is_err() {
            return 0;
        }
        let mut c = Cur(&buf, 0);
        let Some(op) = c.u8() else { continue };
        match op {
            OP_NOTIFY => {
                let (Some(pkg), Some(tag), Some(id), Some(app), Some(summary), Some(body), Some(urg), Some(res)) =
                    (c.s(), c.opt(), c.i32(), c.s(), c.s(), c.s(), c.u8(), c.u8())
                else {
                    continue;
                };
                let key = (pkg.clone(), tag.clone(), id);
                let replaces = ids.get(&key).copied().unwrap_or(0);
                let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
                hints.insert("urgency", Value::U8(urg));
                hints.insert("desktop-entry", Value::from(pkg.as_str()));
                if res != 0 {
                    hints.insert("resident", Value::Bool(true));
                }
                let actions: Vec<&str> = Vec::new();
                let timeout: i32 = if res != 0 { 0 } else { -1 };
                match proxy.call::<_, _, u32>("Notify", &(app.as_str(), replaces, "", summary.as_str(), body.as_str(), actions, hints, timeout)) {
                    Ok(hid) => {
                        ids.insert(key, hid);
                    }
                    Err(e) => eprintln!("aro-notify: Notify failed: {e}"),
                }
            }
            OP_CLOSE => {
                let (Some(pkg), Some(tag), Some(id)) = (c.s(), c.opt(), c.i32()) else { continue };
                if let Some(hid) = ids.remove(&(pkg, tag, id)) {
                    let _: Result<(), _> = proxy.call("CloseNotification", &(hid,));
                }
            }
            OP_CLOSE_ALL => {
                let Some(pkg) = c.s() else { continue };
                let keys: Vec<_> = ids.keys().filter(|k| k.0 == pkg).cloned().collect();
                for k in keys {
                    if let Some(hid) = ids.remove(&k) {
                        let _: Result<(), _> = proxy.call("CloseNotification", &(hid,));
                    }
                }
            }
            _ => {}
        }
    }
}
