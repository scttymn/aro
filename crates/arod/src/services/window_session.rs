//! android.view.IWindowSession: the per-app window session WindowManager hands
//! out from `openSession`. Each `addToDisplayAsUser` gets an input channel
//! (socket pair) and a composer layer; `relayout` returns the layer's
//! SurfaceControl and the window's frames.
use super::surfaceflinger::{Layer, SurfaceFlinger};
use super::{codes, Service};
use crate::aparcel as ap;
use crate::parcelables::{write_activity_window_info, Configuration, Rect};
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

pub struct AppWindow {
    pub window: SIBinder,
    pub layer: Arc<Layer>,
    /// Our end of the input channel; the app holds the other.
    pub input_tx: OwnedFd,
    pub token: SIBinder,
    pub relaid_out: bool,
}

/// The host side of the app's window: current frame size (driven by the
/// compositor's configure events), the windows the app added, and the input
/// channel. Shared between the binder service and the Wayland thread.
pub struct WindowHost {
    pub sf: Arc<SurfaceFlinger>,
    pub dpi: i32,
    pub size: Mutex<(i32, i32)>,
    pub windows: Mutex<Vec<AppWindow>>,
    pub input: Arc<crate::input_channel::InputHub>,
}

pub struct WindowSession {
    pub host: Arc<WindowHost>,
}

const IWINDOW_RESIZED: u32 = 2;

impl WindowHost {
    pub fn new(sf: Arc<SurfaceFlinger>, size: (i32, i32), dpi: i32, input: Arc<crate::input_channel::InputHub>) -> Self {
        WindowHost { sf, dpi, size: Mutex::new(size), windows: Mutex::new(Vec::new()), input }
    }

    pub fn frame(&self) -> Rect {
        let (w, h) = *self.size.lock().unwrap();
        Rect { left: 0, top: 0, right: w, bottom: h }
    }

    /// The desktop resized our toplevel: adopt the size and tell every app
    /// window to relayout at it (IWindow.resized, oneway).
    pub fn resize(&self, w: i32, h: i32) {
        {
            let mut s = self.size.lock().unwrap();
            if *s == (w, h) {
                return;
            }
            *s = (w, h);
        }
        let frame = self.frame();
        let windows: Vec<SIBinder> = self.windows.lock().unwrap().iter().map(|a| a.window.clone()).collect();
        for window in windows {
            if let Err(e) = self.send_resized(&window, frame) {
                log::warn!("window: resized callback failed: {e}");
            }
        }
        log::info!("window: resized to {w}x{h} ({} window(s) notified)", self.windows.lock().unwrap().len());
    }

    fn send_resized(&self, window: &SIBinder, frame: Rect) -> anyhow::Result<()> {
        let proxy = window.as_proxy().ok_or_else(|| anyhow::anyhow!("IWindow is not a proxy"))?;
        let mut d = proxy.prepare_transact(true)?;
        // resized(in WindowRelayoutResult layout, boolean reportDraw, boolean forceLayout,
        //         int displayId, boolean syncWithBuffers, boolean dragResizing)
        d.write_i32(1)?; // typed WindowRelayoutResult present
        self.write_relayout_result(&mut d, frame)?;
        ap::boolean(&mut d, true)?; // reportDraw
        ap::boolean(&mut d, true)?; // forceLayout
        d.write_i32(0)?; // displayId
        ap::boolean(&mut d, false)?; // syncWithBuffers
        ap::boolean(&mut d, false)?; // dragResizing
        proxy.submit_transact(IWINDOW_RESIZED, &d, rsbinder::FLAG_ONEWAY)?;
        Ok(())
    }

    fn find_layer(&self, window: &SIBinder) -> Option<Arc<Layer>> {
        self.windows.lock().unwrap().iter().find(|w| &w.window == window).map(|w| w.layer.clone())
    }

    /// InputChannel: nativeWriteToParcel writes int32(isInitialized=1), then the
    /// size-prefixed android.os.InputChannelCore (name, ParcelFileDescriptor, token).
    fn write_input_channel(p: &mut Parcel, name: &str, fd: &OwnedFd, token: &SIBinder) -> Result<()> {
        p.write_i32(1)?; // isInitialized
        let start = p.data_position();
        p.write_i32(0)?; // InputChannelCore size placeholder
        ap::string16(p, Some(name))?; // @utf8InCpp String, written UTF-16 on the wire
        // ParcelFileDescriptor fd: readParcelable = int32(non-null=1), then
        // ParcelFileDescriptor.readFromParcel = readUniqueParcelFileDescriptor
        // = int32(hasComm=0) + fd object.
        p.write_i32(1)?; // non-null
        p.write_i32(0)?; // hasComm = 0
        p.write_raw_file_descriptor(fd.as_fd())?;
        p.write(&Some(token.clone()))?; // IBinder token

        let end = p.data_position();
        p.set_data_position(start);
        p.write_i32((end - start) as i32)?;
        p.set_data_position(end);
        Ok(())
    }

    /// android.view.SurfaceControl as Java writes it: w, h, present, then libgui's native form.
    #[allow(dead_code)]
    fn write_surface_control(p: &mut Parcel, client: &SIBinder, layer: &Layer) -> Result<()> {
        p.write_i32(layer.width as i32)?;
        p.write_i32(layer.height as i32)?;
        p.write_i32(1)?;
        p.write(&Some(client.clone()))?; // client (ISurfaceComposerClient)
        p.write(&Some(layer.handle.clone()))?; // handle
        p.write_i32(layer.id)?; // layerId
        ap::string16(p, Some(&layer.name))?; // name
        p.write_u32(0)?; // transformHint
        p.write_u32(layer.width)?;
        p.write_u32(layer.height)?;
        p.write_u32(1) // format: PIXEL_FORMAT_RGBA_8888
    }

    /// android.view.InsetsState: no insets, a rectangular display. The
    /// decorations (rounded corners, privacy-indicator bounds, display shape)
    /// are written as real empty values: InsetsState.equals calls .equals on
    /// each of them, so nulls NPE the app the first time the state changes.
    fn write_insets_state(p: &mut Parcel, frame: Rect) -> Result<()> {
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        frame.write(p)?; // mDisplayFrame
        p.write_i32(0)?; // DisplayCutout: NO_CUTOUT
        // mRoundedCorners (typed): RoundedCorners.writeToParcel writes just variant 0
        // for NO_ROUNDED_CORNERS (the corner array only follows variant 1).
        p.write_i32(1)?;
        p.write_i32(0)?;
        frame.write(p)?; // mRoundedCornerFrame
        // mPrivacyIndicatorBounds (typed): Rect[4] static bounds (one per rotation) + rotation
        p.write_i32(1)?;
        p.write_i32(4)?;
        for _ in 0..4 {
            p.write_i32(1)?;
            Rect { left: 0, top: 0, right: 0, bottom: 0 }.write(p)?;
        }
        p.write_i32(0)?;
        // mDisplayShape (typed): a plain rectangular display
        p.write_i32(1)?;
        p.write_i32(0)?; // mType
        ap::string8(p, Some(""))?; // mDisplayUniqueId
        p.write_i32(w)?; // mPhysicalDisplayWidth
        p.write_i32(h)?; // mPhysicalDisplayHeight
        ap::boolean(p, false)?; // mIsRound
        ap::string8(p, Some(""))?; // mSpec
        p.write_f32(1.0)?; // mSpecRatio
        p.write_i32(w)?; // mDisplayWidth
        p.write_i32(h)?; // mDisplayHeight
        p.write_i32(0)?; // mRotation
        p.write_i32(0)?; // mOffsetX
        p.write_i32(0)?; // mOffsetY
        p.write_f32(1.0)?; // mScale
        p.write_i32(0)?; // mSeq
        p.write_i32(0) // no sources
    }

    /// android.view.WindowRelayoutResult body.
    fn write_relayout_result(&self, p: &mut Parcel, frame: Rect) -> Result<()> {
        let (w, h, dpi) = (frame.right - frame.left, frame.bottom - frame.top, self.dpi);
        // ClientWindowFrames
        frame.write(p)?; // frame
        frame.write(p)?; // displayFrame
        frame.write(p)?; // parentFrame
        ap::typed_none(p)?; // attachedFrame
        ap::boolean(p, false)?; // isParentFrameClippedByDisplayCutout
        p.write_f32(1.0)?; // compatScale
        p.write_i32(0)?; // seq
        // MergedConfiguration: global, override, merged
        let cfg = Configuration::desktop(w, h, dpi);
        cfg.write(p)?;
        cfg.write(p)?;
        cfg.write(p)?;
        {
            let start = p.data_position();
            Self::write_insets_state(p, frame)?;
            if std::env::var_os("ARO_DUMP_INSETS").is_some() {
                let (bytes, _) = p.aro_debug_bytes();
                let end = p.data_position();
                let hex: String = bytes[start..end].iter().map(|b| format!("{b:02x}")).collect();
                let _ = std::fs::write("/tmp/claude-1000/-home-scttymn-Work/acedbb5c-a659-4a7b-ad88-ecf6b70d7242/scratchpad/insets.hex", hex);
            }
        }
        // activeControls: InsetsSourceControl.Array (non-null required): typed array + seq
        p.write_i32(1)?;
        p.write_i32(0)?; // empty InsetsSourceControl[]
        p.write_i32(0)?; // seq
        p.write_i32(-1)?; // syncSeqId
        ap::typed(p, |p| write_activity_window_info(p, frame))?; // activityWindowInfo
        ap::boolean(p, false) // usesSyncedInsetsAnimation
    }
}

impl Service for WindowSession {
    const DESCRIPTOR: &'static str = "android.view.IWindowSession";
    const TABLE: &'static [(u32, &'static str)] = codes::IWINDOWSESSION;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        let host = &*self.host;
        let frame = host.frame();
        let (w, h) = (frame.right, frame.bottom);
        match name {
            "addToDisplayAsUser" | "addToDisplay" => {
                let window: Option<SIBinder> = data.read()?;
                let Some(window) = window else { return Ok(false) };
                // LayoutParams follows; its layout is large and not needed yet.
                let layer = host.sf.create_layer("app-window", w as u32, h as u32);
                let mut fds = [0i32; 2];
                if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } != 0 {
                    return Err(rsbinder::StatusCode::Unknown);
                }
                let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
                // Publish the server end so the compositor can inject input events.
                if let Ok(dup) = ours.try_clone() {
                    *host.input.fd.lock().unwrap() = Some(dup);
                }
                let token = super::token::new_token("input-channel");
                log::info!("window: addToDisplay -> layer {} with input channel", layer.id);
                ap::no_exception(reply)?;
                // AddWindowResult (structured parcelable): size, relayoutResult(null), inputChannel, returnCode
                reply.write_i32(1)?; // typed object present
                let start = reply.data_position();
                reply.write_i32(0)?;
                ap::typed_none(reply)?; // relayoutResult
                ap::typed(reply, |p| WindowHost::write_input_channel(p, "aro-window", &theirs, &token))?;
                // ADD_FLAG_APP_VISIBLE (2) | ADD_FLAG_IN_TOUCH_MODE (1): ViewRootImpl
                // reads this as `res`; without APP_VISIBLE the window stays GONE and never draws.
                reply.write_i32(2 | 1)?;
                let end = reply.data_position();
                reply.set_data_position(start);
                reply.write_i32((end - start) as i32)?;
                reply.set_data_position(end);
                host.windows.lock().unwrap().push(AppWindow { window, layer, input_tx: ours, token, relaid_out: false });
                Ok(true)
            }
            "relayout" => {
                let window: Option<SIBinder> = data.read()?;
                let layer = window.as_ref().and_then(|wnd| host.find_layer(wnd));
                let Some(layer) = layer else {
                    log::warn!("window: relayout for unknown window");
                    return Ok(false);
                };
                log::info!("window: relayout -> layer {}", layer.id);
                // In this image the client creates its own SurfaceControl (through our
                // ISurfaceComposerClient) and passes it *in*; the reply is just
                // [result int][WindowRelayoutResult].
                // RELAYOUT_RES_SURFACE_CHANGED (2) | RELAYOUT_RES_FIRST_TIME (1): the
                // client (re)creates its SurfaceControl and BLASTBufferQueue.
                let first = { let mut w = host.windows.lock().unwrap(); let e = w.iter_mut().find(|w| Some(&w.window) == window.as_ref()); e.map(|w| { let f = !w.relaid_out; w.relaid_out = true; f }).unwrap_or(true) };
                let flags = if first { 2 | 1 } else { 0 };
                ap::no_exception(reply)?;
                reply.write_i32(flags)?; // result flags
                reply.write_i32(1)?; // outRelayoutResult present
                host.write_relayout_result(reply, frame)?;
                Ok(true)
            }
            "relayoutAsync" => Ok(true), // oneway
            "getWindowId" => {
                ap::no_exception(reply)?;
                reply.write(&Some(super::token::new_token("window-id")))?;
                Ok(true)
            }
            "remove" | "setInsets" | "prepareFrame" | "finishDrawing" | "updateRequestedVisibleTypes" | "updateAnimatingTypes" | "reportSystemGestureExclusionChanged" | "reportDecorViewGestureInterceptionChanged" | "reportKeepClearAreasChanged" | "setOnBackInvokedCallbackInfo" | "clearTouchableRegion" | "cancelDraw" | "pokeDrawLock" | "updateTapExcludeRegion" | "notifyImeWindowVisibilityChangedFromClient" | "onRectangleOnScreenRequested" | "setWallpaperPosition" | "setWallpaperZoomOut" | "setShouldZoomOutWallpaper" => {
                log::debug!("window: {name} (no-op)");
                ap::no_exception(reply).ok();
                Ok(true)
            }
            "outOfMemory" | "startMovingTask" | "grantEmbeddedWindowFocus" | "moveFocusToAdjacentWindow" | "performDrag" | "dropForAccessibility" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
