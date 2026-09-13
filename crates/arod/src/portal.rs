//! Desktop portal requests. Subscribe before invoking to avoid losing fast replies.
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
const DEST: &str = "org.freedesktop.portal.Desktop";
const ROOT: &str = "/org/freedesktop/portal/desktop";
static SERIAL: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
#[path = "portal_tests.rs"]
mod tests;

pub fn connection(uid: u32) -> anyhow::Result<Connection> {
    Ok(zbus::blocking::connection::Builder::session()?
        .user_id(uid)
        .method_timeout(std::time::Duration::from_secs(10))
        .build()?)
}
pub fn open_file(
    uid: u32,
    app: &str,
    mime: Option<&str>,
) -> anyhow::Result<Option<std::path::PathBuf>> {
    let conn = connection(uid)?;
    let token = format!(
        "aro_{}_{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let sender = conn
        .unique_name()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("{ROOT}/request/{sender}/{token}");
    let request = Proxy::new(&conn, DEST, path.as_str(), "org.freedesktop.portal.Request")?;
    let mut responses = request.receive_signal("Response")?;
    let portal = Proxy::new(&conn, DEST, ROOT, "org.freedesktop.portal.FileChooser")?;
    let mut options = HashMap::<&str, Value>::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert("multiple", Value::from(false));
    if let Some(mime) = mime.filter(|m| *m != "*/*") {
        options.insert("filters", Value::from(vec![(mime, vec![(1u32, mime)])]));
    }
    let handle: OwnedObjectPath =
        portal.call("OpenFile", &("", format!("{app}: Open file"), options))?;
    anyhow::ensure!(
        handle.as_str() == path,
        "portal returned an unexpected request handle"
    );
    let msg = responses
        .next()
        .ok_or_else(|| anyhow::anyhow!("portal disconnected"))?;
    let (code, mut results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    if code != 0 {
        return Ok(None);
    }
    let uris = Vec::<String>::try_from(
        results
            .remove("uris")
            .ok_or_else(|| anyhow::anyhow!("portal returned no files"))?,
    )?;
    let uri = uris
        .first()
        .ok_or_else(|| anyhow::anyhow!("empty selection"))?;
    Ok(Some(url::Url::parse(uri)?.to_file_path().map_err(
        |_| anyhow::anyhow!("portal selection is not a local file"),
    )?))
}

#[derive(Clone, Debug)]
pub struct LocationFix {
    pub latitude: f64,
    pub longitude: f64,
    pub accuracy: f32,
    pub time_ms: i64,
    pub elapsed_ns: i64,
}
/// One consent-gated fix from the location portal (GeoClue backend). The
/// connection is private to this request, and cancellation closes it.
pub fn location(
    uid: u32,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<Option<LocationFix>> {
    location_on_connection(
        connection(uid)?,
        canceled,
        std::time::Duration::from_secs(60),
    )
}

fn location_on_connection(
    conn: Connection,
    canceled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<LocationFix>> {
    if canceled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let timer_conn = conn.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let timer_done = done.clone();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if timer_done.load(Ordering::Relaxed) {
                return;
            }
            if canceled.load(Ordering::Relaxed) || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = timer_conn.close();
    });
    let result = (|| {
        let portal = Proxy::new(&conn, DEST, ROOT, "org.freedesktop.portal.Location")?;
        let mut updates = portal.receive_signal("LocationUpdated")?;
        let token = format!(
            "aro_{}_{}",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let options = HashMap::from([
            ("session_handle_token", Value::from(token.as_str())),
            ("accuracy", Value::from(3u32)),
        ]);
        let session: OwnedObjectPath = portal.call("CreateSession", &(options,))?;
        let sender = conn
            .unique_name()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = format!("{ROOT}/request/{sender}/{token}");
        let request = Proxy::new(&conn, DEST, path.as_str(), "org.freedesktop.portal.Request")?;
        let mut responses = request.receive_signal("Response")?;
        let opts = HashMap::from([("handle_token", Value::from(token.as_str()))]);
        let outcome = (|| {
            let handle: OwnedObjectPath = portal.call("Start", &(&session, "", opts))?;
            anyhow::ensure!(
                handle.as_str() == path,
                "unexpected location request handle"
            );
            let msg = responses
                .next()
                .ok_or_else(|| anyhow::anyhow!("location request ended"))?;
            let (code, _): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
            if code != 0 {
                return Ok(None);
            }
            for msg in &mut updates {
                let (handle, mut values): (OwnedObjectPath, HashMap<String, OwnedValue>) =
                    msg.body().deserialize()?;
                if handle != session {
                    continue;
                }
                let mut number = |key| {
                    values
                        .remove(key)
                        .and_then(|v| f64::try_from(v).ok())
                        .ok_or_else(|| anyhow::anyhow!("missing location field {key}"))
                };
                let latitude = number("Latitude")?;
                let longitude = number("Longitude")?;
                let accuracy = number("Accuracy")? as f32;
                anyhow::ensure!(
                    latitude.is_finite()
                        && (-90.0..=90.0).contains(&latitude)
                        && longitude.is_finite()
                        && (-180.0..=180.0).contains(&longitude)
                        && accuracy.is_finite()
                        && accuracy >= 0.0,
                    "invalid location fix"
                );
                let mut now: libc::timespec = unsafe { std::mem::zeroed() };
                unsafe {
                    libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now);
                }
                let received_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis() as i64;
                let (secs, micros) = values
                    .remove("Timestamp")
                    .and_then(|v| <(u64, u64)>::try_from(v).ok())
                    .ok_or_else(|| anyhow::anyhow!("missing location timestamp"))?;
                anyhow::ensure!(micros < 1_000_000, "invalid location timestamp fraction");
                let time_ms = secs
                    .checked_mul(1000)
                    .and_then(|s| s.checked_add(micros / 1000))
                    .and_then(|t| i64::try_from(t).ok())
                    .ok_or_else(|| anyhow::anyhow!("invalid location timestamp"))?;
                let age_ns = received_ms
                    .saturating_sub(time_ms)
                    .max(0)
                    .saturating_mul(1_000_000);
                let elapsed_ns = (now.tv_sec * 1_000_000_000 + now.tv_nsec)
                    .saturating_sub(age_ns)
                    .max(0);
                return Ok(Some(LocationFix {
                    latitude,
                    longitude,
                    accuracy,
                    time_ms,
                    elapsed_ns,
                }));
            }
            Ok(None)
        })();
        if let Ok(p) = Proxy::new(
            &conn,
            DEST,
            session.as_str(),
            "org.freedesktop.portal.Session",
        ) {
            let _: zbus::Result<()> = p.call("Close", &());
        }
        outcome
    })();
    done.store(true, Ordering::Relaxed);
    result
}

/// The portal's location implementation follows this host preference. Failure
/// to read it is disabled, not implicit consent.
pub fn location_enabled() -> bool {
    std::process::Command::new("timeout")
        .args([
            "2s",
            "gsettings",
            "get",
            "org.gnome.system.location",
            "enabled",
        ])
        .output()
        .is_ok_and(|o| o.status.success() && o.stdout == b"true\n")
}

pub fn open_uri(uid: u32, uri: &str) -> anyhow::Result<bool> {
    let conn = connection(uid)?;
    let token = format!(
        "aro_{}_{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    );
    let sender = conn
        .unique_name()
        .unwrap()
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("{ROOT}/request/{sender}/{token}");
    let request = Proxy::new(&conn, DEST, path.as_str(), "org.freedesktop.portal.Request")?;
    let mut responses = request.receive_signal("Response")?;
    let portal = Proxy::new(&conn, DEST, ROOT, "org.freedesktop.portal.OpenURI")?;
    let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
    let handle: OwnedObjectPath = portal.call("OpenURI", &("", uri, options))?;
    anyhow::ensure!(handle.as_str() == path, "unexpected portal handle");
    let msg = responses
        .next()
        .ok_or_else(|| anyhow::anyhow!("OpenURI portal disconnected"))?;
    let (code, _): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize()?;
    Ok(code == 0)
}
