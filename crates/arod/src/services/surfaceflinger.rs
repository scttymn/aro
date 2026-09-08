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
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
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
}

impl SurfaceFlinger {
    pub fn new(width: u32, height: u32) -> Self {
        SurfaceFlinger { layers: Mutex::new(Vec::new()), next_layer_id: AtomicI32::new(1), width, height, client: Mutex::new(None) }
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
                let vsync_source = data.read_i32()?;
                let registration = data.read_i32()?;
                let _layer: Option<SIBinder> = data.read().unwrap_or(None);
                log::info!("sf: createDisplayEventConnection source={vsync_source} registration={registration:#x}");
                let conn = DisplayEventConnection::new()?;
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
}

impl Service for ComposerLegacy {
    const DESCRIPTOR: &'static str = "android.ui.ISurfaceComposer";
    const TABLE: &'static [(u32, &'static str)] = ISURFACECOMPOSER_LEGACY;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "setTransactionState" => {
                let n = self.transactions.fetch_add(1, Ordering::SeqCst) + 1;
                log::info!("sf: setTransactionState #{n} ({} bytes)", data.data_size());
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
}

const DISPLAY_EVENT_VSYNC: u32 = 0x7673_796e; // fourcc('v','s','y','n')
const FRAME_INTERVAL_NS: i64 = 16_666_667;
const EVENT_SIZE: usize = 224; // sizeof(DisplayEventReceiver::Event) on x86_64

fn now_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

impl DisplayEventConnection {
    fn new() -> Result<Self> {
        let mut fds = [0i32; 2];
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0, fds.as_mut_ptr()) } != 0 {
            return Err(rsbinder::StatusCode::Unknown);
        }
        use std::os::fd::FromRawFd;
        let (receive, send) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        Ok(DisplayEventConnection { receive, send: Arc::new(send), rate: Arc::new(AtomicI32::new(0)), counter: Arc::new(AtomicU32::new(0)) })
    }

    /// One DisplayEventReceiver::Event of type VSYNC.
    fn write_vsync(send: &OwnedFd, count: u32) {
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
        push(&mut e, &FRAME_INTERVAL_NS.to_le_bytes()); // frameInterval
        push(&mut e, &0u32.to_le_bytes()); // preferredFrameTimelineIndex
        push(&mut e, &1u32.to_le_bytes()); // frameTimelinesLength
        push(&mut e, &0u32.to_le_bytes()); // numberQueuedBuffers
        push(&mut e, &[0u8; 4]); // pad
        let vsync_id = count as i64;
        let deadline = now + FRAME_INTERVAL_NS - 2_000_000;
        let present = now + 2 * FRAME_INTERVAL_NS;
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
        p.write_i64(FRAME_INTERVAL_NS)?;
        p.write_u32(0)?; // preferredFrameTimelineIndex
        p.write_u32(1)?; // frameTimelinesLength
        p.write_i64(count as i64)?; // vsyncId
        p.write_i64(now + FRAME_INTERVAL_NS - 2_000_000)?; // deadline
        p.write_i64(now + 2 * FRAME_INTERVAL_NS) // expectedPresentationTime
    }
}

impl Service for DisplayEventConnection {
    const DESCRIPTOR: &'static str = "android.gui.IDisplayEventConnection";
    const TABLE: &'static [(u32, &'static str)] = IDISPLAYEVENTCONNECTION;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "stealReceiveChannel" => {
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
                    let (send, rate_flag, counter) = (self.send.clone(), self.rate.clone(), self.counter.clone());
                    std::thread::spawn(move || {
                        while rate_flag.load(Ordering::SeqCst) > 0 {
                            std::thread::sleep(std::time::Duration::from_nanos(FRAME_INTERVAL_NS as u64));
                            let c = counter.fetch_add(1, Ordering::SeqCst) + 1;
                            Self::write_vsync(&send, c);
                        }
                    });
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "requestNextVsync" => {
                let (send, counter) = (self.send.clone(), self.counter.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(8));
                    let c = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    Self::write_vsync(&send, c);
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
