//! Wire-level tests on a private bus: never use the desktop or real coordinates.
use super::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{Duration, Instant};

const SESSION: &str = "/org/freedesktop/portal/desktop/session/test";

struct Bus(Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Clone, Copy)]
enum Outcome {
    Allow,
    Deny,
    Error,
    Wait,
    Invalid,
}
struct LocationPortal(Outcome);
#[zbus::interface(name = "org.freedesktop.portal.Location")]
impl LocationPortal {
    fn create_session(&self, _options: HashMap<String, OwnedValue>) -> OwnedObjectPath {
        SESSION.try_into().unwrap()
    }

    async fn start(
        &self,
        session: OwnedObjectPath,
        _parent: String,
        options: HashMap<String, OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        if matches!(self.0, Outcome::Error) {
            return Err(zbus::fdo::Error::AccessDenied("test denial".into()));
        }
        let token = <&str>::try_from(options.get("handle_token").unwrap()).unwrap();
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = format!("{ROOT}/request/{sender}/{token}");
        if !matches!(self.0, Outcome::Wait) {
            // Deliberately emit a fix before the consent response and before Start
            // returns: the client must queue it but not expose it on denial.
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap();
            let micros = if matches!(self.0, Outcome::Invalid) {
                1_000_000
            } else {
                stamp.subsec_micros() as u64
            };
            let values = HashMap::from([
                ("Latitude", Value::from(1.25f64)),
                ("Longitude", Value::from(-2.5f64)),
                ("Accuracy", Value::from(500.0f64)),
                ("Timestamp", Value::from((stamp.as_secs(), micros))),
            ]);
            conn.emit_signal(
                None::<&str>,
                ROOT,
                "org.freedesktop.portal.Location",
                "LocationUpdated",
                &(&session, values),
            )
            .await?;
            let code = if matches!(self.0, Outcome::Deny) {
                1u32
            } else {
                0u32
            };
            conn.emit_signal(
                None::<&str>,
                path.as_str(),
                "org.freedesktop.portal.Request",
                "Response",
                &(code, HashMap::<String, Value>::new()),
            )
            .await?;
        }
        Ok(path.try_into().unwrap())
    }
}

struct Session(Arc<AtomicBool>);
#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    fn close(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct Fixture {
    client: Connection,
    _server: Connection,
    closed: Arc<AtomicBool>,
    _bus: Bus,
}
fn fixture(outcome: Outcome) -> Fixture {
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("tests require dbus-daemon"));
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let closed = Arc::new(AtomicBool::new(false));
    let server = zbus::blocking::connection::Builder::address(address.trim())
        .unwrap()
        .name(DEST)
        .unwrap()
        .serve_at(ROOT, LocationPortal(outcome))
        .unwrap()
        .serve_at(SESSION, Session(closed.clone()))
        .unwrap()
        .build()
        .unwrap();
    let client = zbus::blocking::connection::Builder::address(address.trim())
        .unwrap()
        .method_timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    Fixture {
        client,
        _server: server,
        closed,
        _bus: bus,
    }
}

#[test]
fn fast_location_is_delivered_only_after_consent() {
    for outcome in [Outcome::Allow, Outcome::Deny] {
        let f = fixture(outcome);
        let fix = location_on_connection(
            f.client.clone(),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3),
        )
        .unwrap();
        if matches!(outcome, Outcome::Allow) {
            let fix = fix.expect("fast signal must not be lost");
            assert_eq!(
                (fix.latitude, fix.longitude, fix.accuracy),
                (1.25, -2.5, 500.0)
            );
            assert!(fix.elapsed_ns > 0);
        } else {
            assert!(fix.is_none(), "denied requests must discard queued fixes");
        }
        assert!(f.closed.load(Ordering::SeqCst));
    }
}

#[test]
fn location_errors_close_the_session() {
    for outcome in [Outcome::Error, Outcome::Invalid] {
        let f = fixture(outcome);
        assert!(location_on_connection(
            f.client.clone(),
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(3)
        )
        .is_err());
        assert!(f.closed.load(Ordering::SeqCst));
    }
}

#[test]
fn unanswered_location_requests_are_bounded() {
    let f = fixture(Outcome::Wait);
    let start = Instant::now();
    assert!(location_on_connection(
        f.client.clone(),
        Arc::new(AtomicBool::new(false)),
        Duration::from_millis(200)
    )
    .is_err());
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn cancellation_interrupts_a_pending_location_request() {
    let f = fixture(Outcome::Wait);
    let canceled = Arc::new(AtomicBool::new(false));
    let signal = canceled.clone();
    let canceler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        signal.store(true, Ordering::Relaxed);
    });
    let start = Instant::now();
    let result = location_on_connection(f.client.clone(), canceled, Duration::from_secs(10));
    canceler.join().unwrap();
    assert!(result.is_err() || result.unwrap().is_none());
    assert!(start.elapsed() < Duration::from_secs(3));
}
