//! The ARO notification shade: a persistent, Android-style view of the app's
//! notifications, served to a companion Omarchy bar widget.
//!
//! Ephemeral toasts still go to the host's freedesktop daemon (the heads-up
//! alert); the shade is the pull-down list you can open again later, and the
//! home for ongoing notifications. arod is the source of truth: it holds the
//! current notifications and serves them over a line-delimited JSON protocol
//! on a Unix socket. The widget is a thin client — it renders the list and
//! sends `invoke`/`dismiss` back.
//!
//! Wire protocol (one JSON object per line, both directions):
//!   arod  -> widget : {"type":"snapshot","notifications":[Note,...]}
//!                     {"type":"posted","notification":Note}
//!                     {"type":"removed","id":N}
//!   widget -> arod  : {"cmd":"invoke","id":N}
//!                     {"cmd":"dismiss","id":N}
//!                     {"cmd":"dismissAll"}
//! where Note = {id,app,appName,title,body,ongoing,ts}.
use crate::pending_intent::Target;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Re-launch a notification's target (the tap-back dispatch), shared with the
/// freedesktop tap path so a shade tap and a toast tap behave identically.
pub type Fire = Arc<dyn Fn(&Target) + Send + Sync>;
/// Close the host (freedesktop) copy of a notification when it leaves the shade.
pub type CloseHost = Arc<dyn Fn(&str, Option<&str>, i32) + Send + Sync>;

#[derive(Clone, Serialize)]
pub struct Note {
    pub id: u64,
    pub app: String,
    #[serde(rename = "appName")]
    pub app_name: String,
    pub title: String,
    pub body: String,
    pub ongoing: bool,
    pub ts: i64,
    /// (package, tag, android id) — identity for replace/cancel; not sent.
    #[serde(skip)]
    pub key: (String, Option<String>, i32),
    /// Tap-back target; not sent (the widget only asks us to invoke by id).
    #[serde(skip)]
    pub target: Option<Target>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum Event<'a> {
    #[serde(rename = "snapshot")]
    Snapshot { notifications: &'a [Note] },
    #[serde(rename = "posted")]
    Posted { notification: &'a Note },
    #[serde(rename = "removed")]
    Removed { id: u64 },
}

struct Inner {
    next_id: u64,
    notes: Vec<Note>,
    clients: Vec<UnixStream>,
}

pub struct Shade {
    inner: Mutex<Inner>,
    fire: Fire,
    close_host: Mutex<Option<CloseHost>>,
}

impl Shade {
    pub fn new(fire: Fire) -> Arc<Shade> {
        Arc::new(Shade {
            inner: Mutex::new(Inner { next_id: 1, notes: Vec::new(), clients: Vec::new() }),
            fire,
            close_host: Mutex::new(None),
        })
    }

    /// Dismissing from the shade should also clear the host's toast/center copy.
    pub fn set_close_host(&self, f: CloseHost) {
        *self.close_host.lock().unwrap() = Some(f);
    }

    /// Bind the shade socket and accept widget connections. Best-effort: a
    /// missing widget just means an empty client list.
    pub fn serve(self: &Arc<Self>, path: &Path) -> std::io::Result<()> {
        // A stale socket from a previous run would refuse bind; the widget only
        // ever talks to the live arod, so replacing it is safe.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        log::info!("shade: serving at {}", path.display());
        let this = self.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                this.on_connect(stream);
            }
        });
        Ok(())
    }

    fn on_connect(self: &Arc<Self>, stream: UnixStream) {
        // Send the current list, then keep a write handle for future broadcasts
        // and spawn a reader for this client's commands.
        let mut inner = self.inner.lock().unwrap();
        let line = serde_json::to_string(&Event::Snapshot { notifications: &inner.notes }).unwrap_or_default();
        let mut w = match stream.try_clone() {
            Ok(w) => w,
            Err(e) => { log::warn!("shade: clone client: {e}"); return; }
        };
        let _ = writeln!(w, "{line}");
        inner.clients.push(w);
        drop(inner);

        let this = self.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                this.handle_command(&line);
            }
        });
    }

    fn handle_command(&self, line: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
        let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        match cmd {
            "invoke" => {
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    let target = self.inner.lock().unwrap().notes.iter().find(|n| n.id == id).and_then(|n| n.target.clone());
                    if let Some(t) = target {
                        log::info!("shade: invoke {id} -> {t:?}");
                        (self.fire)(&t);
                    }
                }
            }
            "dismiss" => {
                if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                    self.dismiss(id);
                }
            }
            "dismissAll" => self.dismiss_all(),
            other => log::debug!("shade: unknown command {other:?}"),
        }
    }

    /// Add or replace a notification (Android re-posts the same (pkg,tag,id) to
    /// update in place) and tell every connected widget.
    #[allow(clippy::too_many_arguments)]
    pub fn post(&self, key: (String, Option<String>, i32), app: &str, app_name: &str, title: &str, body: &str, ongoing: bool, target: Option<Target>) {
        let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let mut inner = self.inner.lock().unwrap();
        let id = match inner.notes.iter().position(|n| n.key == key) {
            Some(i) => inner.notes[i].id, // keep the id so the widget updates in place
            None => {
                let id = inner.next_id;
                inner.next_id += 1;
                id
            }
        };
        let note = Note { id, app: app.into(), app_name: app_name.into(), title: title.into(), body: body.into(), ongoing, ts, key: key.clone(), target };
        match inner.notes.iter().position(|n| n.key == key) {
            Some(i) => inner.notes[i] = note.clone(),
            None => inner.notes.push(note.clone()),
        }
        let line = serde_json::to_string(&Event::Posted { notification: &note }).unwrap_or_default();
        Self::broadcast(&mut inner, &line);
    }

    /// Remove a notification the app itself cancelled.
    pub fn remove(&self, key: &(String, Option<String>, i32)) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(i) = inner.notes.iter().position(|n| &n.key == key) {
            let id = inner.notes.remove(i).id;
            let line = serde_json::to_string(&Event::Removed { id }).unwrap_or_default();
            Self::broadcast(&mut inner, &line);
        }
    }

    /// Remove every notification for a package (cancelAll).
    pub fn remove_all(&self, app: &str) {
        let mut inner = self.inner.lock().unwrap();
        let ids: Vec<u64> = inner.notes.iter().filter(|n| n.key.0 == app).map(|n| n.id).collect();
        inner.notes.retain(|n| n.key.0 != app);
        for id in ids {
            let line = serde_json::to_string(&Event::Removed { id }).unwrap_or_default();
            Self::broadcast(&mut inner, &line);
        }
    }

    /// User dismissed one from the shade: drop it and clear the host copy.
    fn dismiss(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        let Some(i) = inner.notes.iter().position(|n| n.id == id) else { return };
        let note = inner.notes.remove(i);
        let line = serde_json::to_string(&Event::Removed { id }).unwrap_or_default();
        Self::broadcast(&mut inner, &line);
        drop(inner);
        if let Some(close) = self.close_host.lock().unwrap().clone() {
            close(&note.key.0, note.key.1.as_deref(), note.key.2);
        }
    }

    fn dismiss_all(&self) {
        let notes: Vec<Note> = {
            let mut inner = self.inner.lock().unwrap();
            let notes = std::mem::take(&mut inner.notes);
            for n in &notes {
                let line = serde_json::to_string(&Event::Removed { id: n.id }).unwrap_or_default();
                Self::broadcast(&mut inner, &line);
            }
            notes
        };
        if let Some(close) = self.close_host.lock().unwrap().clone() {
            for n in &notes {
                close(&n.key.0, n.key.1.as_deref(), n.key.2);
            }
        }
    }

    /// Write one line to every client, dropping any that have gone away.
    fn broadcast(inner: &mut Inner, line: &str) {
        inner.clients.retain_mut(|c| writeln!(c, "{line}").is_ok());
    }
}
