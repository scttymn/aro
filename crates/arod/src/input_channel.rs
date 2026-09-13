//! Android input, host side. ARO is the input publisher: when the desktop sends
//! a pointer event to an app's Hyprland window, we encode it as an Android
//! `InputMessage` and write it to the window's input channel (a SEQPACKET
//! socket whose other end the app's `InputConsumer` reads). The struct layout
//! mirrors `frameworks/native/include/input/InputTransport.h` for x86_64; the
//! app reads it as a raw struct, so offsets and padding must match exactly.
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, Ordering};
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
const AXIS_VSCROLL: u64 = 9;
const AXIS_HSCROLL: u64 = 10;

fn axis_bit(axis: u64) -> u64 { 0x8000_0000_0000_0000 >> axis }

/// Shared handle to the (single, for M3) app window's input channel. The window
/// session stores the server end here when the app adds its window; the
/// compositor writes events to it.
#[derive(Default)]
pub struct InputHub {
    fd: Mutex<Option<OwnedFd>>,
    focused: AtomicBool,
    window_id: AtomicI32,
    seq: AtomicU32,
    down_time: AtomicI64,
}

pub fn now_ns() -> i64 {
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

#[derive(Clone, Copy, Debug, Default)]
pub struct KeyInput {
    pub action: i32, pub code: i32, pub scan: i32, pub meta: i32,
    pub repeat: i32, pub down_time: i64, pub flags: i32,
}

fn encode_key(key: KeyInput, seq: u32, event_time: i64) -> Vec<u8> {
    let mut b = Buf::new();
    b.u32(0); // InputMessage::Type::KEY
    b.u32(seq);
    b.i32(next_event_id());
    b.u32(0);
    b.i64(event_time);
    b.i32(-1); // virtual keyboard, with the KCM published by InputService
    b.i32(0x101); // SOURCE_KEYBOARD
    b.i32(0); // display
    b.pad(32); // unsigned event HMAC
    for value in [key.action, key.flags, key.code, key.scan, key.meta, key.repeat] { b.i32(value); }
    b.u32(0);
    b.i64(key.down_time);
    b.0
}

impl InputHub {
    pub fn select_channel(&self, fd: Option<OwnedFd>, window_id: i32) {
        let mut guard = self.fd.lock().unwrap();
        if self.window_id.swap(window_id, Ordering::SeqCst) != window_id {
            if self.focused.load(Ordering::SeqCst) {
                if let Some(old) = guard.as_ref() { self.write_focus(old, false); }
                if let Some(new) = fd.as_ref() { self.write_focus(new, true); }
            }
        }
        *guard = fd;
    }

    pub fn set_focus(&self, focused: bool) {
        self.focused.store(focused, Ordering::SeqCst);
        if let Some(fd) = self.fd.lock().unwrap().as_ref() { self.write_focus(fd, focused); }
    }

    fn write_focus(&self, fd: &OwnedFd, focused: bool) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1).max(1);
        let mut b = Buf::new();
        b.u32(3); // FOCUS
        b.u32(seq);
        b.i32(next_event_id());
        b.u32(u32::from(focused)); // bool + three padding bytes
        b.i32(0); // Android 17 FocusDirection::UNDEFINED
        unsafe { libc::send(fd.as_raw_fd(), b.0.as_ptr().cast(), b.0.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL); }
    }

    pub fn send_key(&self, key: KeyInput) -> bool {
        let guard = self.fd.lock().unwrap();
        let Some(fd) = guard.as_ref() else { return false };
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).wrapping_add(1).max(1);
        let bytes = encode_key(key, seq, now_ns());
        let n = unsafe { libc::send(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
        if n != bytes.len() as isize {
            log::warn!("input: key send failed: {}", std::io::Error::last_os_error());
            return false;
        }
        log::debug!("input: key action={} code={} meta={:#x} repeat={}", key.action, key.code, key.meta, key.repeat);
        true
    }

    /// Encode and send one single-pointer MotionEvent (a touch). `x`,`y` are in
    /// the app's window coordinates, while `raw_x`,`raw_y` are display coordinates.
    /// Returns false if there is no channel yet.
    pub fn send_motion(&self, action: i32, x: f32, y: f32, raw_x: f32, raw_y: f32) -> bool {
        self.send_pointer(action, x, y, raw_x, raw_y, None)
    }

    pub fn send_scroll(&self, x: f32, y: f32, raw_x: f32, raw_y: f32, vertical: f32, horizontal: f32) -> bool {
        self.send_pointer(8, x, y, raw_x, raw_y, Some((vertical, horizontal)))
    }

    fn send_pointer(&self, action: i32, x: f32, y: f32, raw_x: f32, raw_y: f32, scroll: Option<(f32, f32)>) -> bool {
        let mut guard = self.fd.lock().unwrap();
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
        b.i32(if scroll.is_some() { 0x2002 } else { SOURCE_TOUCHSCREEN }); // mouse or touch source
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
        // PointerCoords holds display coordinates; the window transform
        // subtracts a child window's origin to produce local coordinates.
        for v in [1.0f32, 0.0, 0.0, 1.0, x - raw_x, y - raw_y] {
            b.f32(v);
        }
        b.f32(1.0); // xPrecision
        b.f32(1.0); // yPrecision
        b.f32(if scroll.is_some() { x } else { f32::NAN }); // xCursorPosition
        b.f32(if scroll.is_some() { y } else { f32::NAN }); // yCursorPosition
        // Raw (display) transform: identity.
        for v in [1.0f32, 0.0, 0.0, 1.0, 0.0, 0.0] {
            b.f32(v);
        }
        // pointers[0] (aligned 8; body is already 8-aligned here).
        b.align8();
        // PointerProperties { id, toolType }.
        b.i32(0);
        b.u32(if scroll.is_some() { 3 } else { TOOL_TYPE_FINGER });
        // PointerCoords { bits, values[30], isResampled, empty[7] }.
        // Android BitSet64 numbers axes from the most-significant bit.
        let bits = axis_bit(AXIS_X) | axis_bit(AXIS_Y) | if scroll.is_some() {
            axis_bit(AXIS_VSCROLL) | axis_bit(AXIS_HSCROLL)
        } else { axis_bit(AXIS_PRESSURE) };
        b.u64(bits);
        let mut vals = [0f32; 30];
        vals[0] = raw_x;
        vals[1] = raw_y;
        if let Some((vertical, horizontal)) = scroll {
            vals[2] = vertical;
            vals[3] = horizontal;
        } else {
            vals[2] = if action == ACTION_UP || action == ACTION_CANCEL { 0.0 } else { 1.0 };
        }
        for v in vals {
            b.f32(v);
        }
        b.0.push(0); // isResampled
        b.pad(7); // empty[7]

        if std::env::var_os("ARO_INPUT_DEBUG").is_some() {
            let g = |o: usize| u32::from_le_bytes([b.0[o], b.0[o+1], b.0[o+2], b.0[o+3]]);
            let f = |o: usize| f32::from_le_bytes([b.0[o], b.0[o+1], b.0[o+2], b.0[o+3]]);
            log::info!("input dbg: len={} action@68={} ptr.id@168={} ptr.tool@172={} bits@176={:#x} vx@184={} vy@188={} vp@192={}",
                b.0.len(), g(68), g(168), g(172), u64::from_le_bytes(b.0[176..184].try_into().unwrap()), f(184), f(188), f(192));
        }
        let rc = unsafe { libc::send(fd.as_raw_fd(), b.0.as_ptr() as *const _, b.0.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            log::warn!("input: send failed: {err}");
            if err.raw_os_error() == Some(libc::EPIPE) {
                *guard = None;
            }
            return false;
        }
        if rc as usize != b.0.len() {
            log::warn!("input: short send {rc} of {} bytes", b.0.len());
        }
        log::debug!("input: motion action={action} local=({x:.0},{y:.0}) raw=({raw_x:.0},{raw_y:.0}) seq={seq} ({} bytes)", b.0.len());
        true
    }

    /// Drain any FINISHED acks the app sent back, so the socket doesn't fill.
    pub fn drain_finished(&self) {
        let guard = self.fd.lock().unwrap();
        let Some(fd) = guard.as_ref() else { return };
        let mut buf = [0u8; 512];
        loop {
            let n = unsafe { libc::recv(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len(), libc::MSG_DONTWAIT) };
            if n > 0 { log::debug!("input: drained {n} bytes from channel"); }
            if n <= 0 {
                break;
            }
            if n as usize >= 8 {
                let ty = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                if ty == TYPE_FINISHED {
                    log::debug!("input: got FINISHED ack from app (event consumed)");
                }
            }
        }
    }
}

fn next_event_id() -> i32 {
    static ID: AtomicU32 = AtomicU32::new(1);
    (ID.fetch_add(1, Ordering::Relaxed) & 0x3fff_ffff) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;
    #[test]
    fn pointer_axes_use_android_bit_order_and_window_coordinates() {
        let hub = InputHub::default();
        let (tx, rx) = UnixDatagram::pair().unwrap();
        rx.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
        hub.select_channel(Some(tx.into()), 1);
        assert!(hub.send_motion(ACTION_DOWN, 10.0, 20.0, 310.0, 420.0));
        let mut bytes = [0u8; 512];
        assert_eq!(rx.recv(&mut bytes).unwrap(), 312);
        let axis = |bytes: &[u8], a: u64| {
            let bits = u64::from_le_bytes(bytes[176..184].try_into().unwrap());
            assert_ne!(bits & axis_bit(a), 0);
            let prior = if a == 0 { 0 } else { bits & (!0u64 << (64-a)) };
            let offset = 184 + 4 * prior.count_ones() as usize;
            f32::from_le_bytes(bytes[offset..offset+4].try_into().unwrap())
        };
        assert_eq!(axis(&bytes, 0), 310.0);
        assert_eq!(axis(&bytes, 1), 420.0);
        assert_eq!(axis(&bytes, 2), 1.0);
        assert_eq!(f32::from_le_bytes(bytes[120..124].try_into().unwrap()) + axis(&bytes,0), 10.0);
        assert_eq!(f32::from_le_bytes(bytes[124..128].try_into().unwrap()) + axis(&bytes,1), 20.0);
        assert!(hub.send_scroll(10.0, 20.0, 310.0, 420.0, -3.0, 2.0));
        assert_eq!(rx.recv(&mut bytes).unwrap(), 312);
        assert_eq!(axis(&bytes, 9), -3.0);
        assert_eq!(axis(&bytes, 10), 2.0);
        assert_eq!(i32::from_le_bytes(bytes[28..32].try_into().unwrap()), 0x2002);
    }

    #[test]
    fn android17_key_message_preserves_fields_and_timestamps() {
        let key = KeyInput { action:1, code:29, scan:30, meta:0x1001, repeat:3, down_time:1234, flags:8 };
        let b = encode_key(key, 7, 5678);
        let i32_at = |at: usize| i32::from_le_bytes(b[at..at+4].try_into().unwrap());
        assert_eq!(b.len(), 104);
        assert_eq!(i32_at(0), 0);
        assert_eq!(i32_at(4), 7);
        assert_eq!(i32_at(24), -1);
        assert_eq!(i32_at(68), 1);
        assert_eq!(i32_at(76), 29);
        assert_eq!(i32_at(84), 0x1001);
        assert_eq!(i32_at(88), 3);
        assert_eq!(i64::from_le_bytes(b[16..24].try_into().unwrap()), 5678);
        assert_eq!(i64::from_le_bytes(b[96..104].try_into().unwrap()), 1234);
    }
    #[test]
    fn focus_moves_between_channels_and_includes_android17_direction() {
        let hub = InputHub::default();
        let (a, ar) = UnixDatagram::pair().unwrap();
        let (b, br) = UnixDatagram::pair().unwrap();
        ar.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
        br.set_read_timeout(Some(std::time::Duration::from_millis(100))).unwrap();
        hub.select_channel(Some(a.into()), 1);
        hub.set_focus(true);
        hub.select_channel(Some(b.into()), 2);
        for (socket, focus) in [(&ar,1u8), (&ar,0), (&br,1)] {
            let mut bytes = [0u8; 32];
            assert_eq!(socket.recv(&mut bytes).unwrap(), 20);
            assert_eq!(bytes[0], 3);
            assert_eq!(bytes[12], focus);
            assert_eq!(&bytes[16..20], &[0;4]);
        }
    }
}
