//! First-use consent and optional dependency installation on the real host.
//! The private socket is created before entering namespaces; it is not an app API.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Serialize, Deserialize)]
enum Message {
    Request(String),
    Cancel,
}

pub struct HostSetup {
    socket: Mutex<UnixStream>,
    child: Option<Child>,
}
impl HostSetup {
    /// No threads are started in the supervisor: session::enter must stay single-threaded.
    pub fn spawn() -> Result<Self> {
        let (client, server) = UnixStream::pair()?;
        client.set_read_timeout(Some(Duration::from_millis(100)))?;
        let child = Command::new(std::env::current_exe()?)
            .arg("location-host")
            .stdin(Stdio::from(OwnedFd::from(server.try_clone()?)))
            .stdout(Stdio::from(OwnedFd::from(server)))
            .spawn()
            .context("start host location helper")?;
        Ok(Self {
            socket: Mutex::new(client),
            child: Some(child),
        })
    }

    pub fn prepare(&self, package: &str, canceled: &AtomicBool) -> Result<bool> {
        let mut socket = loop {
            if canceled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            match self.socket.try_lock() {
                Ok(lock) => break lock,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(_) => anyhow::bail!("location helper lock poisoned"),
            }
        };
        send(&mut *socket, &Message::Request(package.to_owned()))?;
        let mut bytes = Vec::new();
        let mut sent_cancel = false;
        loop {
            if canceled.load(Ordering::Relaxed) && !sent_cancel {
                send(&mut *socket, &Message::Cancel)?;
                sent_cancel = true;
            }
            let mut byte = [0];
            match socket.read(&mut byte) {
                Ok(0) => anyhow::bail!("host location helper disconnected"),
                Ok(_) if byte[0] == b'\n' => {
                    return Ok(serde_json::from_slice::<bool>(&bytes)? && !sent_cancel)
                }
                Ok(_) => {
                    bytes.push(byte[0]);
                    anyhow::ensure!(bytes.len() < 4096, "oversized helper response");
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl Drop for HostSetup {
    fn drop(&mut self) {
        if let Ok(socket) = self.socket.get_mut() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        // Let an already-authorized package transaction finish; never kill pacman halfway.
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}
fn send(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Internal helper entry point. EOF cancels a dialog; an approved install finishes.
pub fn serve() -> Result<()> {
    let mut input = std::io::stdin().lock();
    let mut current: Option<(Arc<AtomicBool>, std::thread::JoinHandle<()>)> = None;
    let decisions = Arc::new(Mutex::new(HashMap::new()));
    let result = (|| -> Result<()> {
        loop {
            let mut line = Vec::new();
            loop {
                let mut byte = [0];
                if input.read(&mut byte)? == 0 {
                    return Ok(());
                }
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
                anyhow::ensure!(line.len() < 4096, "oversized location request");
            }
            match serde_json::from_slice::<Message>(&line)? {
                Message::Cancel => {
                    if let Some((canceled, _)) = &current {
                        canceled.store(true, Ordering::Relaxed);
                    }
                }
                Message::Request(package) => {
                    if let Some((_, worker)) = current.take() {
                        let _ = worker.join();
                    }
                    let canceled = Arc::new(AtomicBool::new(false));
                    let signal = canceled.clone();
                    let decisions = decisions.clone();
                    let worker = std::thread::spawn(move || {
                        let ready = prepare(
                            &mut Desktop,
                            &package,
                            &signal,
                            &mut decisions.lock().unwrap(),
                        )
                        .unwrap_or_else(|e| {
                            log::warn!("location setup failed: {e:#}");
                            false
                        });
                        let _ = send(&mut std::io::stdout().lock(), &ready);
                    });
                    current = Some((canceled, worker));
                }
            }
        }
    })();
    if let Some((canceled, worker)) = current {
        canceled.store(true, Ordering::Relaxed);
        let _ = worker.join();
    }
    result
}

trait Backend {
    fn installed(&mut self) -> bool;
    fn enabled(&mut self) -> bool;
    fn confirm(
        &mut self,
        package: &str,
        install: bool,
        enable: bool,
        canceled: &AtomicBool,
    ) -> Result<bool>;
    fn install(&mut self, canceled: &AtomicBool) -> Result<bool>;
    fn enable(&mut self) -> Result<bool>;
}
fn prepare(
    backend: &mut impl Backend,
    package: &str,
    canceled: &AtomicBool,
    decisions: &mut HashMap<String, bool>,
) -> Result<bool> {
    // Names are display text only, never command fragments; reject control/markup surprises.
    anyhow::ensure!(
        !package.is_empty()
            && package.len() <= 255
            && package
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_'),
        "invalid location requester"
    );
    if canceled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if let Some(allowed) = decisions.get(package) {
        // A later host disable must remain disabled; no remembered grant may re-enable it.
        return Ok(*allowed && backend.installed() && backend.enabled());
    }
    let installed = backend.installed();
    let enabled = backend.enabled();
    let accepted = backend.confirm(package, !installed, !enabled, canceled)?;
    if canceled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if !accepted {
        decisions.insert(package.into(), false);
        return Ok(false);
    }
    if !installed && !backend.install(canceled)? {
        return Ok(false);
    }
    if canceled.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if !enabled && !backend.enable()? {
        return Ok(false);
    }
    let ready = backend.installed() && backend.enabled();
    if ready {
        decisions.insert(package.into(), true);
    }
    Ok(ready)
}

struct Desktop;
impl Backend for Desktop {
    fn installed(&mut self) -> bool {
        Command::new("/usr/bin/pacman")
            .args(["-Q", "geoclue"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    fn enabled(&mut self) -> bool {
        crate::portal::location_enabled()
    }
    fn confirm(
        &mut self,
        package: &str,
        install: bool,
        enable: bool,
        canceled: &AtomicBool,
    ) -> Result<bool> {
        let mut text = format!("Allow {package} to use your location?");
        if install {
            text.push_str("\n\nLocation support (GeoClue) will be installed. Administrator authentication may be required.");
        }
        if enable {
            text.push_str("\n\nThis will enable desktop location services.");
        }
        text.push_str("\n\nDeny leaves location unavailable to this app.");
        let mut child = Command::new("/usr/bin/zenity")
            .args([
                "--question",
                "--no-markup",
                "--default-cancel",
                "--timeout=60",
                "--title=Location permission",
                "--ok-label=Allow",
                "--cancel-label=Deny",
                "--text",
                &text,
            ])
            .stdout(Stdio::null())
            .spawn()?;
        loop {
            if canceled.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
            if let Some(status) = child.try_wait()? {
                return Ok(status.success());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    fn install(&mut self, canceled: &AtomicBool) -> Result<bool> {
        // Only create installation state after consent. Serialize across ARO sessions,
        // and recheck because another session may have installed GeoClue meanwhile.
        let dir = std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is unset")?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(std::path::Path::new(&dir).join("aro-location-install.lock"))?;
        loop {
            if canceled.load(Ordering::Relaxed) {
                return Ok(false);
            }
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::WouldBlock {
                return Err(e.into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if self.installed() {
            return Ok(true);
        }
        if canceled.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let mut command = Command::new("/usr/bin/pkexec");
        if std::path::Path::new("/usr/share/omarchy/bin/omarchy").exists() {
            command.args(["/usr/share/omarchy/bin/omarchy", "pkg", "add", "geoclue"]);
        } else {
            command.args([
                "/usr/bin/pacman",
                "-S",
                "--needed",
                "--noconfirm",
                "geoclue",
            ]);
        }
        // stdout is reserved for the private protocol. Never interrupt a transaction
        // after consent, even if the Android request gets canceled meanwhile.
        let status = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(std::io::stderr()))
            .status()?;
        Ok(status.success() && self.installed())
    }
    fn enable(&mut self) -> Result<bool> {
        let status = Command::new("/usr/bin/timeout")
            .args([
                "5s",
                "/usr/bin/gsettings",
                "set",
                "org.gnome.system.location",
                "enabled",
                "true",
            ])
            .stdout(Stdio::null())
            .status()?;
        Ok(status.success() && self.enabled())
    }
}

#[cfg(test)]
#[path = "location_setup_tests.rs"]
mod tests;
