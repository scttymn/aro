//! ARO's composer, as the app sees it: the two SurfaceFlinger binders
//! ("SurfaceFlingerAIDL" = android.gui.ISurfaceComposer, "SurfaceFlinger" =
//! the legacy android.ui.ISurfaceComposer carrying setTransactionState),
//! per-client ISurfaceComposerClient objects that create layers, and
//! IDisplayEventConnection objects that deliver vsync over a socket pair.
//!
//! Layers are recorded here; presenting their buffers to Wayland is the
//! composer's job (compositor.rs, later in M3).
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub const PHYSICAL_DISPLAY_ID: i64 = 0x4a2b_0001; // arbitrary stable id for the one display

/// A layer created by an app through ISurfaceComposerClient.createSurface.
pub struct Layer {
    pub id: i32,
    pub name: String,
    pub handle: SIBinder,
    pub width: u32,
    pub height: u32,
}

pub struct SurfaceFlinger {
    pub layers: Mutex<Vec<Arc<Layer>>>,
    next_layer_id: AtomicI32,
    pub width: u32,
    pub height: u32,
    /// The ISurfaceComposerClient binder handed to apps (set at publish time).
    pub client: Mutex<Option<SIBinder>>,
    /// Vsync period, from the host output's refresh rate.
    pub frame_interval_ns: i64,
    /// The app's BufferReleaseChannel producer end (from layer_state_t.bufferReleaseChannel):
    /// BLASTBufferQueue's blocked dequeueBuffer polls the consumer end, so releases go here.
    pub release_channel: Arc<Mutex<Option<OwnedFd>>>,
}

impl SurfaceFlinger {
    pub fn new(width: u32, height: u32, frame_interval_ns: i64) -> Self {
        SurfaceFlinger { layers: Mutex::new(Vec::new()), next_layer_id: AtomicI32::new(1), width, height, client: Mutex::new(None), frame_interval_ns, release_channel: Arc::new(Mutex::new(None)) }
    }

    pub fn create_layer(&self, name: &str, width: u32, height: u32) -> Arc<Layer> {
        let id = self.next_layer_id.fetch_add(1, Ordering::SeqCst);
        let handle = super::token::new_token("layer-handle");
        let layer = Arc::new(Layer { id, name: name.to_string(), handle, width, height });
        self.layers.lock().unwrap().push(layer.clone());
        log::info!("sf: layer {id} {name:?} {width}x{height}");
        layer
    }
}

/// android.gui.ISurfaceComposer ("SurfaceFlingerAIDL"); codes from libs/gui/aidl/android/gui/ISurfaceComposer.aidl order.
pub static ISURFACECOMPOSER: &[(u32, &str)] = &[
    (1, "bootFinished"),
    (2, "createDisplayEventConnection"),
    (3, "createConnection"),
    (4, "createVirtualDisplay"),
    (5, "destroyVirtualDisplay"),
    (6, "getPhysicalDisplayIds"),
    (7, "getPhysicalDisplayToken"),
    (8, "getSupportedFrameTimestamps"),
    (9, "setPowerMode"),
    (10, "getDisplayStats"),
    (11, "getDisplayState"),
    (12, "getStaticDisplayInfo"),
    (13, "getDynamicDisplayInfoFromId"),
    (14, "getDynamicDisplayInfoFromToken"),
    (15, "getDisplayNativePrimaries"),
    (34, "getCompositionPreference"),
    (38, "getProtectedContentSupport"),
    (39, "isWideColorDisplay"),
];

pub struct ComposerAidl {
    pub sf: Arc<SurfaceFlinger>,
    pub client: SIBinder,
    pub display_token: SIBinder,
}

impl Service for ComposerAidl {
    const DESCRIPTOR: &'static str = "android.gui.ISurfaceComposer";
    const TABLE: &'static [(u32, &'static str)] = ISURFACECOMPOSER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "bootFinished" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "createConnection" => {
                ap::no_exception(reply)?;
                reply.write(&Some(self.client.clone()))?;
                Ok(true)
            }
            "createDisplayEventConnection" => {
                // AIDL order in this build: (VsyncSource vsyncSource, @nullable
                // IBinder layerHandle, EventRegistration eventRegistration) — an
                // earlier assumption of (source, registration, layer) mis-parsed
                // the args and left Choreographer without a working vsync source.
                let vsync_source = data.read_i32()?;
                let _layer: Option<SIBinder> = data.read().unwrap_or(None);
                let registration = data.read_i32().unwrap_or(0);
                log::info!("sf: createDisplayEventConnection source={vsync_source} registration={registration:#x}");
                let conn = DisplayEventConnection::new(self.sf.frame_interval_ns)?;
                let binder = super::binder_of(conn);
                ap::no_exception(reply)?;
                reply.write(&Some(binder))?;
                Ok(true)
            }
            "getPhysicalDisplayIds" => {
                ap::no_exception(reply)?;
                ap::long_array(reply, Some(&[PHYSICAL_DISPLAY_ID]))?;
                Ok(true)
            }
            "getPhysicalDisplayToken" => {
                let id = data.read_i64()?;
                ap::no_exception(reply)?;
                reply.write(&if id == PHYSICAL_DISPLAY_ID { Some(self.display_token.clone()) } else { None })?;
                Ok(true)
            }
            "getProtectedContentSupport" | "isWideColorDisplay" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// Legacy android.ui.ISurfaceComposer ("SurfaceFlinger"): hand-written C++
/// interface; replies carry no exception header.
// Legacy android.ui.ISurfaceComposer codes (ISurfaceComposer.h ISurfaceComposerTag):
// BOOT_FINISHED=1 .. SET_TRANSACTION_STATE=8.
pub static ISURFACECOMPOSER_LEGACY: &[(u32, &str)] = &[(8, "setTransactionState")];

pub struct ComposerLegacy {
    pub sf: Arc<SurfaceFlinger>,
    pub transactions: AtomicU32,
    pub gralloc: Arc<crate::services::allocator::Gralloc>,
    pub presenter: Option<crate::compositor::Presenter>,
}

/// A buffer an app posted in setTransactionState, with what we need to give
/// it back: BLASTBufferQueue keys releases by the GraphicBuffer's own id and
/// frame number, through the per-buffer `releaseBufferListener` binder.
#[derive(Clone)]
pub struct PostedBuffer {
    pub gralloc_id: u64,
    pub width: u32,
    pub height: u32,
    pub gb_id: u64,
    pub frame_number: u64,
    pub release_listener: Option<SIBinder>,
    pub channel: Arc<Mutex<Option<OwnedFd>>>,
}

const GB01: i32 = 0x4742_3031; // GraphicBuffer flatten magic 'GB01'
const FLAT_OBJ: usize = 24; // sizeof(flat_binder_object)
const GB_HEADER_INTS: usize = 13;

/// Find the posted buffer in a transaction parcel. We locate our gralloc
/// handle by its "AROB" magic and read the GraphicBuffer + BufferData around
/// it (Parcel::write(Flattenable): [len][fdCount][data padded][fd objects]).
fn parse_posted_buffer(data: &mut Parcel) -> Option<PostedBuffer> {
    let save = data.data_position();
    let bytes = data.aro_debug_bytes().0;
    let rd = |o: usize| -> i32 { i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) };
    let magic = super::allocator::HANDLE_MAGIC.to_le_bytes();
    let mut found = None;
    let mut i = GB_HEADER_INTS * 4 + 8;
    while i + 44 <= bytes.len() {
        if bytes[i..i + 4] == magic {
            found = Some(i);
        }
        i += 4;
    }
    let p = found?;
    let h = p - GB_HEADER_INTS * 4; // GraphicBuffer flatten header
    if rd(h) != GB01 {
        log::warn!("sf: AROB handle without GB01 header ({:#x})", rd(h));
        return None;
    }
    let (w, hgt) = (rd(h + 4) as u32, rd(h + 8) as u32);
    let gb_id = ((rd(h + 28) as u32 as u64) << 32) | rd(h + 32) as u32 as u64;
    let num_fds = rd(h + 40) as usize;
    let num_ints = rd(h + 44) as usize;
    let gralloc_id = rd(p + 36) as u32 as u64 | ((rd(p + 40) as u32 as u64) << 32);
    // Flattenable framing: [len][fdCount] precede `h`; data is padded to 4, then fdCount objects.
    let len = rd(h - 8) as usize;
    let fd_count = rd(h - 4) as usize;
    if len != (GB_HEADER_INTS + num_ints) * 4 || fd_count != num_fds {
        log::warn!("sf: GraphicBuffer framing mismatch len={len} fds={fd_count} (ints={num_ints} numFds={num_fds})");
    }
    let mut pos = h + ((len + 3) & !3) + fd_count * FLAT_OBJ;
    // BufferData continues: bool acquireFence [+ Flattenable], u64 frameNumber, binder releaseBufferListener.
    data.set_data_position(pos);
    let has_fence = data.read_i32().ok()? != 0;
    if has_fence {
        let flen = data.read_i32().ok()? as usize;
        let ffds = data.read_i32().ok()? as usize;
        pos = data.data_position() + ((flen + 3) & !3) + ffds * FLAT_OBJ;
        data.set_data_position(pos);
    }
    let frame_number = data.read_u64().ok()?;
    let release_listener: Option<SIBinder> = data.read().ok().flatten();
    data.set_data_position(save);
    Some(PostedBuffer { gralloc_id, width: w, height: hgt, gb_id, frame_number, release_listener, channel: Arc::new(Mutex::new(None)) })
}

/// Look for a BufferReleaseChannel producer endpoint in a transaction: an fd
/// object preceded by a UTF-16 name (ProducerEndpoint::writeToParcel), which
/// is what `Transaction::setBufferReleaseChannel` sends. Buffer and fence fds
/// are excluded by shape (they follow Flattenable data, not a string).
fn find_release_channel(data: &mut Parcel) -> Option<(String, OwnedFd)> {
    let (bytes, objs) = data.aro_debug_bytes();
    let rd = |o: usize| -> i32 { i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]) };
    for &o in &objs {
        let o = o as usize;
        if o + FLAT_OBJ > bytes.len() || rd(o) != 0x6664_2a85 {
            continue; // not BINDER_TYPE_FD
        }
        // A ProducerEndpoint name: [i32 len][utf16 chars + NUL, padded] right before the fd object.
        // Walk back: find a plausible length prefix whose padded utf16 payload ends at `o`.
        let mut name = None;
        for len in 1..=64usize {
            let payload = ((len + 1) * 2 + 3) & !3;
            if o < payload + 4 {
                break;
            }
            let lp = o - payload - 4;
            if rd(lp) as usize == len {
                let mut units = Vec::with_capacity(len);
                for k in 0..len {
                    units.push(u16::from_le_bytes([bytes[lp + 4 + k * 2], bytes[lp + 5 + k * 2]]));
                }
                if let Ok(sname) = String::from_utf16(&units) {
                    if sname.chars().all(|c| !c.is_control()) {
                        name = Some(sname);
                        break;
                    }
                }
            }
        }
        let Some(name) = name else { continue };
        let fd = rd(o + 8);
        let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if dup < 0 {
            log::warn!("sf: dup release channel fd {fd}: {}", std::io::Error::last_os_error());
            return None;
        }
        return Some((name, unsafe { OwnedFd::from_raw_fd(dup) }));
    }
    None
}

/// Write one release into the BufferReleaseChannel: Message::flatten =
/// Fence (u32 numFds=0 for NO_FENCE) then bufferId lo/hi, frame lo/hi, maxAcquired.
fn write_release_channel(fd: &OwnedFd, pb: &PostedBuffer) -> std::io::Result<()> {
    let mut m = Vec::with_capacity(24);
    m.extend_from_slice(&0u32.to_le_bytes()); // fence: no fds
    m.extend_from_slice(&(pb.gb_id as u32).to_le_bytes());
    m.extend_from_slice(&((pb.gb_id >> 32) as u32).to_le_bytes());
    m.extend_from_slice(&(pb.frame_number as u32).to_le_bytes());
    m.extend_from_slice(&((pb.frame_number >> 32) as u32).to_le_bytes());
    m.extend_from_slice(&u32::MAX.to_le_bytes()); // maxAcquiredBufferCount: nullopt
    let rc = unsafe { libc::send(fd.as_raw_fd(), m.as_ptr() as *const _, m.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// ITransactionCompletedListener.onReleaseBuffer(ReleaseCallbackId, Fence, maxAcquiredBufferCount, releasePreviousBuffer), oneway.
pub fn release_buffer(pb: &PostedBuffer) {
    if let Some(ch) = pb.channel.lock().unwrap().as_ref() {
        match write_release_channel(ch, pb) {
            Ok(()) => log::debug!("sf: release channel <- gb={:#x} frame={}", pb.gb_id, pb.frame_number),
            Err(e) => log::warn!("sf: release channel write failed: {e}"),
        }
    }
    let Some(listener) = &pb.release_listener else { return };
    let r = (|| -> anyhow::Result<()> {
        let proxy = listener.as_proxy().ok_or_else(|| anyhow::anyhow!("release listener is not a proxy"))?;
        let mut d = proxy.prepare_transact(true)?;
        d.write_i32(1)?; // writeParcelable: non-null
        d.write_u64(pb.gb_id)?; // ReleaseCallbackId.bufferId
        d.write_u64(pb.frame_number)?; // ReleaseCallbackId.framenumber
        // Fence (Flattenable): len 4, no fds, numFds 0 == NO_FENCE
        d.write_i32(4)?;
        d.write_i32(0)?;
        d.write_u32(0)?;
        d.write_u32(u32::MAX)?; // currentMaxAcquiredBufferCount: nullopt (std::nullopt in C++)
        d.write_bool(false)?; // releasePreviousBuffer: bool (false)
        proxy.submit_transact(2, &d, rsbinder::FLAG_ONEWAY)?; // ON_RELEASE_BUFFER
        Ok(())
    })();
    match r {
        Ok(()) => log::debug!("sf: released buffer gb={:#x} frame={}", pb.gb_id, pb.frame_number),
        Err(e) => log::warn!("sf: onReleaseBuffer failed: {e}"),
    }
}

impl Service for ComposerLegacy {
    const DESCRIPTOR: &'static str = "android.ui.ISurfaceComposer";
    const TABLE: &'static [(u32, &'static str)] = ISURFACECOMPOSER_LEGACY;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "setTransactionState" => {
                let n = self.transactions.fetch_add(1, Ordering::SeqCst) + 1;
                // The posted buffer is a flattened GraphicBuffer; its ARO native handle
                // carries "AROB" magic + our buffer id, which we scan for rather than
                // parsing the whole layer_state_t.
                if let Some((name, fd)) = find_release_channel(data) {
                    log::info!("sf: buffer release channel from {name:?}");
                    *self.sf.release_channel.lock().unwrap() = Some(fd);
                }
                if let Some(mut pb) = parse_posted_buffer(data) {
                    pb.channel = self.sf.release_channel.clone();
                    log::info!("sf: setTransactionState #{n} posts ARO buffer {} ({}x{}) gb={:#x} frame={} release={}", pb.gralloc_id, pb.width, pb.height, pb.gb_id, pb.frame_number, pb.release_listener.is_some());
                    match (&self.presenter, self.gralloc.buffers.lock().unwrap().get(&pb.gralloc_id).cloned()) {
                        (Some(p), Some(buf)) => p.present(buf, pb),
                        _ => release_buffer(&pb), // nothing to show it on: hand it straight back
                    }
                } else {
                    log::debug!("sf: setTransactionState #{n} ({} bytes, no buffer)", data.data_size());
                }
                reply.write_i32(0)?; // status_t OK
                Ok(true)
            }
            "getSchedulingPolicy" => {
                reply.write_i32(0)?; // status
                reply.write_i32(1)?; // SchedulingPolicy parcelable size marker? (structured: int policy, int priority)
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// android.gui.ISurfaceComposerClient (one per app connection).
pub static ISURFACECOMPOSERCLIENT: &[(u32, &str)] = &[(1, "createSurface"), (2, "clearLayerFrameStats"), (3, "getLayerFrameStats"), (4, "mirrorSurface"), (5, "mirrorDisplay"), (6, "getSchedulingPolicy")];

pub struct ComposerClient {
    pub sf: Arc<SurfaceFlinger>,
}

impl Service for ComposerClient {
    const DESCRIPTOR: &'static str = "android.gui.ISurfaceComposerClient";
    const TABLE: &'static [(u32, &'static str)] = ISURFACECOMPOSERCLIENT;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "createSurface" => {
                // createSurface(@utf8InCpp String name, int flags, @nullable IBinder parent, in LayerMetadata metadata)
                let lname: Option<String> = data.read()?;
                let flags = data.read_i32()?;
                let _parent: Option<SIBinder> = data.read().unwrap_or(None);
                let layer = self.sf.create_layer(lname.as_deref().unwrap_or("?"), self.sf.width, self.sf.height);
                log::info!("sf: createSurface {:?} flags={flags:#x} -> layer {}", lname, layer.id);
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // writeParcelable non-null marker
                // CreateSurfaceResult (structured parcelable): size, handle, layerId, layerName, transformHint
                let start = reply.data_position();
                reply.write_i32(0)?;
                reply.write(&Some(layer.handle.clone()))?;
                reply.write_i32(layer.id)?;
                ap::string16(reply, Some(&layer.name))?;
                reply.write_i32(0)?; // transformHint
                let end = reply.data_position();
                reply.set_data_position(start);
                reply.write_i32((end - start) as i32)?;
                reply.set_data_position(end);
                Ok(true)
            }
            "getSchedulingPolicy" => {
                ap::no_exception(reply)?;
                reply.write_i32(12)?; // parcelable size: 4 + 2 ints
                reply.write_i32(0)?; // policy
                reply.write_i32(0)?; // priority
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// android.gui.IDisplayEventConnection: vsync delivery over a BitTube
/// (socket pair; the app reads DisplayEventReceiver::Event records).
pub static IDISPLAYEVENTCONNECTION: &[(u32, &str)] = &[(1, "stealReceiveChannel"), (2, "setVsyncRate"), (3, "requestNextVsync"), (4, "getLatestVsyncEventData"), (5, "getSchedulingPolicy")];

pub struct DisplayEventConnection {
    receive: OwnedFd,
    send: Arc<OwnedFd>,
    /// >0: continuous vsync at that divisor; 0: only on request.
    rate: Arc<AtomicI32>,
    counter: Arc<AtomicU32>,
    interval_ns: i64,
}

const DISPLAY_EVENT_VSYNC: u32 = 0x7673_796e; // fourcc('v','s','y','n')
const EVENT_SIZE: usize = 224; // sizeof(DisplayEventReceiver::Event) on x86_64

fn now_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

impl DisplayEventConnection {
    fn new(interval_ns: i64) -> Result<Self> {
        let mut fds = [0i32; 2];
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0, fds.as_mut_ptr()) } != 0 {
            return Err(rsbinder::StatusCode::Unknown);
        }
        use std::os::fd::FromRawFd;
        let (receive, send) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(DisplayEventConnection { receive, send: Arc::new(send), rate: Arc::new(AtomicI32::new(0)), counter: Arc::new(AtomicU32::new(0)), interval_ns })
    }

    /// One DisplayEventReceiver::Event of type VSYNC.
    fn write_vsync(send: &OwnedFd, count: u32, interval: i64) {
        let now = now_ns();
        let mut e = Vec::with_capacity(EVENT_SIZE);
        let push = |v: &mut Vec<u8>, b: &[u8]| v.extend_from_slice(b);
        push(&mut e, &DISPLAY_EVENT_VSYNC.to_le_bytes());
        push(&mut e, &[0u8; 4]); // pad to 8
        push(&mut e, &(PHYSICAL_DISPLAY_ID as u64).to_le_bytes());
        push(&mut e, &now.to_le_bytes());
        // VSync
        push(&mut e, &count.to_le_bytes());
        push(&mut e, &[0u8; 4]); // pad: VsyncEventData is 8-aligned
        push(&mut e, &interval.to_le_bytes()); // frameInterval
        push(&mut e, &0u32.to_le_bytes()); // preferredFrameTimelineIndex
        push(&mut e, &1u32.to_le_bytes()); // frameTimelinesLength
        push(&mut e, &0u32.to_le_bytes()); // numberQueuedBuffers
        push(&mut e, &[0u8; 4]); // pad
        let vsync_id = count as i64;
        let deadline = now + interval - 2_000_000;
        let present = now + 2 * interval;
        push(&mut e, &vsync_id.to_le_bytes());
        push(&mut e, &deadline.to_le_bytes());
        push(&mut e, &present.to_le_bytes());
        e.resize(EVENT_SIZE, 0);
        let rc = unsafe { libc::send(send.as_raw_fd(), e.as_ptr() as *const _, e.len(), libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL) };
        if rc < 0 {
            log::debug!("sf: vsync send failed: {}", std::io::Error::last_os_error());
        }
    }

    fn write_vsync_event_data(&self, p: &mut Parcel) -> Result<()> {
        let now = now_ns();
        let count = self.counter.load(Ordering::SeqCst);
        let interval = self.interval_ns;
        p.write_i64(interval)?;
        p.write_u32(0)?; // preferredFrameTimelineIndex
        p.write_u32(1)?; // frameTimelinesLength
        p.write_i64(count as i64)?; // vsyncId
        p.write_i64(now + interval - 2_000_000)?; // deadline
        p.write_i64(now + 2 * interval) // expectedPresentationTime
    }
}

impl Service for DisplayEventConnection {
    const DESCRIPTOR: &'static str = "android.gui.IDisplayEventConnection";
    const TABLE: &'static [(u32, &'static str)] = IDISPLAYEVENTCONNECTION;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "stealReceiveChannel" => {
                log::debug!("sf: DEC.stealReceiveChannel");
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // out parcelable present
                reply.write_raw_file_descriptor(self.receive.as_fd())?; // BitTube: receive fd
                reply.write_raw_file_descriptor(self.send.as_fd())?; // then send fd
                Ok(true)
            }
            "setVsyncRate" => {
                let rate = data.read_i32()?;
                self.rate.store(rate, Ordering::SeqCst);
                log::info!("sf: setVsyncRate {rate}");
                if rate > 0 {
                    // Continuous vsync until the rate is set back to 0.
                    let (send, rate_flag, counter, interval) = (self.send.clone(), self.rate.clone(), self.counter.clone(), self.interval_ns);
                    std::thread::spawn(move || {
                        while rate_flag.load(Ordering::SeqCst) > 0 {
                            std::thread::sleep(std::time::Duration::from_nanos(interval as u64));
                            let c = counter.fetch_add(1, Ordering::SeqCst) + 1;
                            Self::write_vsync(&send, c, interval);
                        }
                    });
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "requestNextVsync" => {
                log::debug!("sf: DEC.requestNextVsync");
                let (send, counter, interval) = (self.send.clone(), self.counter.clone(), self.interval_ns);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_nanos((interval / 2) as u64));
                    let c = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    Self::write_vsync(&send, c, interval);
                });
                Ok(true)
            }
            "getLatestVsyncEventData" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?; // parcelable present
                self.write_vsync_event_data(reply)?;
                Ok(true)
            }
            "getSchedulingPolicy" => {
                ap::no_exception(reply)?;
                reply.write_i32(12)?;
                reply.write_i32(0)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
