//! Android input, host side. ARO is the input publisher: when the desktop sends
//! a pointer event to an app's Hyprland window, we encode it as an Android
//! `InputMessage` and write it to the window's input channel (a SEQPACKET
//! socket whose other end the app's `InputConsumer` reads). The struct layout
//! mirrors `frameworks/native/include/input/InputTransport.h` for x86_64; the
//! app reads it as a raw struct, so offsets and padding must match exactly.
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;

// InputMessage::Type
const TYPE_MOTION: u32 = 1;
const TYPE_FINISHED: u32 = 2;

// AMOTION_EVENT_ACTION_*
pub const ACTION_DOWN: i32 = 0;
pub const ACTION_UP: i32 = 1;
pub const ACTION_MOVE: i32 = 2;
pub const ACTION_CANCEL: i32 = 3;

const SOURCE_TOUCHSCREEN: i32 = 0x0000_1002;
const TOOL_TYPE_FINGER: u32 = 1;

const AXIS_X: u64 = 0;
const AXIS_Y: u64 = 1;
const AXIS_PRESSURE: u64 = 2;

/// Shared handle to the (single, for M3) app window's input channel. The window
/// session stores the server end here when the app adds its window; the
/// compositor writes events to it.
#[derive(Default)]
pub struct InputHub {
    pub fd: Mutex<Option<OwnedFd>>,
    seq: AtomicU32,
    down_time: AtomicI64,
}

fn now_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

struct Buf(Vec<u8>);
impl Buf {
    fn new() -> Self {
        Buf(Vec::with_capacity(312))
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn pad(&mut self, n: usize) {
        self.0.resize(self.0.len() + n, 0);
    }
    fn align8(&mut self) {
        while self.0.len() % 8 != 0 {
            self.0.push(0);
        }
    }
}

impl InputHub {
    /// Encode and send one single-pointer MotionEvent (a touch). `x`,`y` are in
    /// the app's window coordinates. Returns false if there is no channel yet.
    pub fn send_motion(&self, action: i32, x: f32, y: f32) -> bool {
        let guard = self.fd.lock().unwrap();
        let Some(fd) = guard.as_ref() else {
            log::warn!("input: no input channel yet; dropping motion action={action}");
            return false;
        };
        let event_time = now_ns();
        if action == ACTION_DOWN {
            self.down_time.store(event_time, Ordering::SeqCst);
        }
        let down_time = self.down_time.load(Ordering::SeqCst);
        let seq = self.seq.fetch_add(1, Ordering::SeqCst).wrapping_add(1).max(1);
        let mut b = Buf::new();
        // Header: type, seq.
        b.u32(TYPE_MOTION);
        b.u32(seq);
        // Body::Motion (offsets relative to body start).
        b.i32(next_event_id()); // eventId
        b.u32(1); // pointerCount
        b.align8();
        b.i64(event_time); // eventTime
        b.i32(0); // deviceId (virtual)
        b.i32(SOURCE_TOUCHSCREEN); // source
        b.i32(0); // displayId
        b.pad(32); // hmac[32]
        b.i32(action); // action
        b.i32(0); // actionButton
        b.i32(0); // flags
        b.i32(0); // metaState
        b.i32(0); // buttonState
        b.0.push(0); // classification (MotionClassification::NONE)
        b.pad(3); // empty2[3]
        b.i32(0); // edgeFlags
        b.align8();
        b.i64(down_time); // downTime
        // Window transform: identity rotation/scale with translation carrying the
        // point (dsdx, dtdx, dtdy, dsdy, tx, ty). getX = dsdx*rawX + tx, and the
        // per-pointer raw values read as zero on this GSI build, so the point is
        // delivered through tx/ty here and txRaw/tyRaw below. See note in send_motion.
        for v in [1.0f32, 0.0, 0.0, 1.0, x, y] {
            b.f32(v);
        }
        b.f32(1.0); // xPrecision
        b.f32(1.0); // yPrecision
        b.f32(f32::NAN); // xCursorPosition
        b.f32(f32::NAN); // yCursorPosition
        // Raw (display) transform, same translation so getRawX/Y also resolve.
        for v in [1.0f32, 0.0, 0.0, 1.0, x, y] {
            b.f32(v);
        }
        // pointers[0] (aligned 8; body is already 8-aligned here).
        b.align8();
        // PointerProperties { id, toolType }.
        b.i32(0);
        b.u32(TOOL_TYPE_FINGER);
        // PointerCoords { bits, values[30], isResampled, empty[7] }.
        let bits = (1u64 << AXIS_X) | (1 << AXIS_Y) | (1 << AXIS_PRESSURE);
        b.u64(bits);
        let mut vals = [0f32; 30];
        vals[0] = x; // X
        vals[1] = y; // Y
        vals[2] = if action == ACTION_UP { 0.0 } else { 1.0 }; // PRESSURE
        for v in vals {
            b.f32(v);
        }
        b.0.push(0); // isResampled
        b.pad(7); // empty[7]

        if std::env::var_os("ARO_INPUT_DEBUG").is_some() {
            let g = |o: usize| u32::from_le_bytes([b.0[o], b.0[o+1], b.0[o+2], b.0[o+3]]);
            let f = |o: usize| f32::from_le_bytes([b.0[o], b.0[o+1], b.0[o+2], b.0[o+3]]);
            log::info!("input dbg: len={} action@68={} ptr.id@168={} ptr.tool@172={} bits@176={:#x} vx@184={} vy@188={} vp@192={}",
                b.0.len(), g(68), g(168), g(172), g(176), f(184), f(188), f(192));
        }
        let rc = unsafe { libc::send(fd.as_raw_fd(), b.0.as_ptr() as *const _, b.0.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
        if rc < 0 {
            log::warn!("input: send failed: {}", std::io::Error::last_os_error());
            return false;
        }
        if rc as usize != b.0.len() {
            log::warn!("input: short send {rc} of {} bytes", b.0.len());
        }
        log::debug!("input: motion action={action} ({x:.0},{y:.0}) seq={seq} ({} bytes)", b.0.len());
        true
    }

    /// Drain any FINISHED acks the app sent back, so the socket doesn't fill.
    pub fn drain_finished(&self) {
        let guard = self.fd.lock().unwrap();
        let Some(fd) = guard.as_ref() else { return };
        let mut buf = [0u8; 512];
        loop {
            let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), libc::MSG_DONTWAIT) };
            if n > 0 { log::info!("input: drained {n} bytes from channel"); }
            if n <= 0 {
                break;
            }
            if n as usize >= 8 {
                let ty = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                if ty == TYPE_FINISHED {
                    log::info!("input: got FINISHED ack from app (event consumed)");
                }
            }
        }
    }
}

fn next_event_id() -> i32 {
    static ID: AtomicU32 = AtomicU32::new(1);
    (ID.fetch_add(1, Ordering::Relaxed) & 0x3fff_ffff) as i32
}
