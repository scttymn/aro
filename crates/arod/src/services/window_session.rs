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

#[derive(Clone)]
pub struct WindowCanvas {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

pub struct AppWindow {
    pub window: SIBinder,
    pub layer: Arc<Layer>,
    /// Our end of the input channel; the app holds the other.
    pub input_tx: OwnedFd,
    #[allow(dead_code)]
    pub token: SIBinder,
    pub relaid_out: bool,
    pub is_child: bool,
    pub wtype: i32,
    pub pos: Mutex<(i32, i32)>,
    pub size: Mutex<(i32, i32)>,
    pub frame: Mutex<Rect>,
    pub canvas: Mutex<Option<WindowCanvas>>,
    pub tag: Mutex<Option<String>>,
    pub surface_layer_id: Mutex<Option<i32>>,
}

/// The host side of the app's window: current frame size (driven by the
/// compositor's configure events), the windows the app added, and the input
/// channel. Shared between the binder service and the Wayland thread.
pub struct WindowHost {
    pub sf: Arc<SurfaceFlinger>,
    pub dpi: i32,
    /// Host output scale: compositor sizes are logical pixels, the app
    /// renders physical ones (logical * scale).
    pub scale: i32,
    pub size: Mutex<(i32, i32)>,
    pub windows: Mutex<Vec<AppWindow>>,
    pub input: Arc<crate::input_channel::InputHub>,
}

pub struct WindowSession {
    pub host: Arc<WindowHost>,
}

const IWINDOW_RESIZED: u32 = 2;

impl WindowHost {
    pub fn new(sf: Arc<SurfaceFlinger>, size: (i32, i32), dpi: i32, scale: i32, input: Arc<crate::input_channel::InputHub>) -> Self {
        WindowHost { sf, dpi, scale: scale.max(1), size: Mutex::new(size), windows: Mutex::new(Vec::new()), input }
    }

    pub fn frame(&self) -> Rect {
        let (w, h) = *self.size.lock().unwrap();
        Rect { left: 0, top: 0, right: w, bottom: h }
    }

    /// The desktop resized our toplevel (logical pixels): adopt the physical
    /// size and tell every app window to relayout at it (IWindow.resized, oneway).
    pub fn resize(&self, w: i32, h: i32) {
        if std::env::var_os("ARO_NO_RESIZE").is_some() {
            return; // debug: keep the first frame size, never push resized()
        }
        let (w, h) = (w * self.scale, h * self.scale);
        {
            let mut s = self.size.lock().unwrap();
            if *s == (w, h) {
                return;
            }
            *s = (w, h);
        }
        let display_frame = self.frame();
        let windows: Vec<(SIBinder, Rect, bool)> = {
            self.windows.lock().unwrap().iter().map(|a| (a.window.clone(), *a.frame.lock().unwrap(), a.is_child)).collect()
        };
        for (window, win_frame, is_child) in windows {
            let f = if is_child { win_frame } else { display_frame };
            if let Err(e) = self.send_resized(&window, f, display_frame) {
                log::warn!("window: resized callback failed: {e}");
            }
        }
        log::info!("window: resized to {w}x{h} ({} window(s) notified)", self.windows.lock().unwrap().len());
    }

    fn send_resized(&self, window: &SIBinder, frame: Rect, display_frame: Rect) -> anyhow::Result<()> {
        let proxy = window.as_proxy().ok_or_else(|| anyhow::anyhow!("IWindow is not a proxy"))?;
        let mut d = proxy.prepare_transact(true)?;
        // resized(in WindowRelayoutResult layout, boolean reportDraw, boolean forceLayout,
        //         int displayId, boolean syncWithBuffers, boolean dragResizing)
        d.write_i32(1)?; // typed WindowRelayoutResult present
        self.write_relayout_result(&mut d, frame, display_frame)?;
        ap::boolean(&mut d, true)?; // reportDraw
        ap::boolean(&mut d, true)?; // forceLayout
        d.write_i32(0)?; // displayId
        ap::boolean(&mut d, false)?; // syncWithBuffers
        ap::boolean(&mut d, false)?; // dragResizing
        proxy.submit_transact(IWINDOW_RESIZED, &d, rsbinder::FLAG_ONEWAY)?;
        Ok(())
    }

    pub fn refresh_input_fd(&self) {
        let windows = self.windows.lock().unwrap();
        if let Some(top) = windows.last() {
            if let Ok(dup) = top.input_tx.try_clone() {
                log::info!("window: set active input channel to layer {}", top.layer.id);
                *self.input.fd.lock().unwrap() = Some(dup);
                return;
            }
        }
        *self.input.fd.lock().unwrap() = None;
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
    pub fn write_relayout_result(&self, p: &mut Parcel, frame: Rect, display_frame: Rect) -> Result<()> {
        let (w, h, dpi) = (display_frame.right - display_frame.left, display_frame.bottom - display_frame.top, self.dpi);
        // ClientWindowFrames
        frame.write(p)?; // frame
        display_frame.write(p)?; // displayFrame
        display_frame.write(p)?; // parentFrame
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
            Self::write_insets_state(p, display_frame)?;
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
        ap::typed(p, |p| write_activity_window_info(p, display_frame))?; // activityWindowInfo
        ap::boolean(p, false) // usesSyncedInsetsAnimation
    }
}

impl Service for WindowSession {
    const DESCRIPTOR: &'static str = "android.view.IWindowSession";
    const TABLE: &'static [(u32, &'static str)] = codes::IWINDOWSESSION;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        let host = &*self.host;
        let display_frame = host.frame();
        let (host_w, host_h) = (display_frame.right, display_frame.bottom);
        match name {
            "addToDisplayAsUser" | "addToDisplay" => {
                let window: Option<SIBinder> = data.read()?;
                let Some(window) = window else { return Ok(false) };
                let save_pos = data.data_position();
                let mut wtype = 1;
                let mut x = 0;
                let mut y = 0;
                let mut req_w = -1;
                let mut req_h = -1;
                let mut flags = 0;
                if let Ok(has_attrs) = data.read_i32() {
                    if has_attrs != 0 {
                        req_w = data.read_i32().unwrap_or(-1);
                        req_h = data.read_i32().unwrap_or(-1);
                        x = data.read_i32().unwrap_or(0);
                        y = data.read_i32().unwrap_or(0);
                        wtype = data.read_i32().unwrap_or(1);
                        flags = data.read_i32().unwrap_or(0);
                        let priv_flags = data.read_i32().unwrap_or(0);
                        let _soft = data.read_i32().unwrap_or(0);
                        let _cutout = data.read_i32().unwrap_or(0);
                        let gravity = data.read_i32().unwrap_or(0);
                        let _hmargin = data.read_f32().unwrap_or(0.0);
                        let _vmargin = data.read_f32().unwrap_or(0.0);
                        let _format = data.read_i32().unwrap_or(0);
                        let _anims = data.read_i32().unwrap_or(0);
                        let _alpha = data.read_f32().unwrap_or(1.0);
                        let _dim = data.read_f32().unwrap_or(0.0);
                        let _sbright = data.read_f32().unwrap_or(0.0);
                        let _bbright = data.read_f32().unwrap_or(0.0);
                        let _rot = data.read_i32().unwrap_or(0);
                        let _token: Option<SIBinder> = data.read().unwrap_or(None);
                        let _ctx_token: Option<SIBinder> = data.read().unwrap_or(None);
                        let _pkg: Option<String> = data.read().unwrap_or(None);
                        let title: Option<String> = if let Ok(kind) = data.read_i32() {
                            if kind == 1 {
                                data.read().unwrap_or(None)
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        log::info!("window: addToDisplay attrs: type={wtype} title={title:?} pos=({x},{y}) size={req_w}x{req_h} grav={gravity:#x} flags={flags:#x} priv={priv_flags:#x}");
                    }
                }
                data.set_data_position(save_pos);
                let has_base = host.windows.lock().unwrap().iter().any(|w| !w.is_child);
                let is_dialog = has_base && (wtype == 2 || (flags & 2) != 0 || wtype == 1003);
                let is_popup = wtype >= 1000 && wtype < 2000 && !is_dialog;
                let is_child = is_popup || is_dialog;
                let win_w = if is_dialog { host_w } else if req_w > 0 { req_w } else if is_popup { 392 } else { host_w };
                let win_h = if is_dialog { host_h } else if req_h > 0 { req_h } else if is_popup { 192 } else { host_h };
                let initial_frame = if is_popup {
                    Rect { left: x, top: y, right: x + win_w, bottom: y + win_h }
                } else {
                    display_frame
                };
                let layer_name = if is_popup { "popup-window" } else if is_dialog { "dialog-window" } else { "app-window" };
                let layer = host.sf.create_layer(layer_name, win_w as u32, win_h as u32);
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
                log::info!("window: addToDisplay -> layer {} (child={is_child} dialog={is_dialog}) with input channel", layer.id);
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
                host.windows.lock().unwrap().push(AppWindow {
                    window,
                    layer,
                    input_tx: ours,
                    token,
                    relaid_out: false,
                    is_child,
                    wtype,
                    pos: Mutex::new((if is_dialog { 0 } else { x }, if is_dialog { 0 } else { y })),
                    size: Mutex::new((win_w, win_h)),
                    frame: Mutex::new(initial_frame),
                    canvas: Mutex::new(None),
                    tag: Mutex::new(None),
                    surface_layer_id: Mutex::new(None),
                });
                host.refresh_input_fd();
                Ok(true)
            }
            "relayout" => {
                let window: Option<SIBinder> = data.read()?;
                let layer = window.as_ref().and_then(|wnd| host.find_layer(wnd));
                let Some(layer) = layer else {
                    log::warn!("window: relayout for unknown window");
                    return Ok(false);
                };
                let save_pos = data.data_position();
                let has_attrs = data.read_i32().unwrap_or(0) != 0;
                let (cur_w, cur_h, cur_x, cur_y, cur_wtype);
                let mut flags = 0;
                if has_attrs {
                    cur_w = data.read_i32().unwrap_or(0);
                    cur_h = data.read_i32().unwrap_or(0);
                    cur_x = data.read_i32().unwrap_or(0);
                    cur_y = data.read_i32().unwrap_or(0);
                    cur_wtype = data.read_i32().unwrap_or(0);
                    flags = data.read_i32().unwrap_or(0);
                    let _priv_flags = data.read_i32().unwrap_or(0);
                    let _soft = data.read_i32().unwrap_or(0);
                    let _cutout = data.read_i32().unwrap_or(0);
                    let gravity = data.read_i32().unwrap_or(0);
                    log::info!("window: relayout -> layer {} attrs: type={cur_wtype} pos=({cur_x},{cur_y}) size={cur_w}x{cur_h} grav={gravity:#x} flags={flags:#x}", layer.id);
                } else {
                    cur_w = data.read_i32().unwrap_or(0);
                    cur_h = data.read_i32().unwrap_or(0);
                    cur_x = 0;
                    cur_y = 0;
                    cur_wtype = 0;
                    log::info!("window: relayout -> layer {} (no attrs), req={cur_w}x{cur_h}", layer.id);
                }
                data.set_data_position(save_pos);
                let (win_frame, win_display_frame) = {
                    let mut windows = host.windows.lock().unwrap();
                    let has_base = windows.iter().any(|w| !w.is_child && Some(&w.window) != window.as_ref());
                    let display_frame = host.frame();
                    if let Some(app_win) = windows.iter_mut().find(|w| Some(&w.window) == window.as_ref()) {
                        if has_attrs {
                            if cur_wtype != 0 { app_win.wtype = cur_wtype; }
                            let is_dialog = has_base && (app_win.wtype == 2 || (flags & 2) != 0 || app_win.wtype == 1003);
                            let is_popup = app_win.wtype >= 1000 && app_win.wtype < 2000 && !is_dialog;
                            app_win.is_child = is_popup || is_dialog;
                            if is_dialog {
                                *app_win.pos.lock().unwrap() = (0, 0);
                                *app_win.size.lock().unwrap() = (host_w, host_h);
                                *app_win.frame.lock().unwrap() = display_frame;
                            } else if is_popup {
                                *app_win.pos.lock().unwrap() = (cur_x, cur_y);
                                let win_w = if cur_w > 0 { cur_w } else { 392 };
                                let win_h = if cur_h > 0 { cur_h } else { 192 };
                                *app_win.size.lock().unwrap() = (win_w, win_h);
                                *app_win.frame.lock().unwrap() = Rect { left: cur_x, top: cur_y, right: cur_x + win_w, bottom: cur_y + win_h };
                            } else {
                                *app_win.pos.lock().unwrap() = (0, 0);
                                *app_win.size.lock().unwrap() = (host_w, host_h);
                                *app_win.frame.lock().unwrap() = display_frame;
                            }
                        } else if cur_w > 0 && cur_h > 0 {
                            let is_dialog = app_win.is_child && *app_win.pos.lock().unwrap() == (0, 0);
                            if !is_dialog {
                                app_win.size.lock().unwrap().0 = cur_w;
                                app_win.size.lock().unwrap().1 = cur_h;
                                if app_win.is_child {
                                    let (px, py) = *app_win.pos.lock().unwrap();
                                    *app_win.frame.lock().unwrap() = Rect { left: px, top: py, right: px + cur_w, bottom: py + cur_h };
                                }
                            }
                        }
                        let f = *app_win.frame.lock().unwrap();
                        (f, display_frame)
                    } else {
                        (display_frame, display_frame)
                    }
                };
                let first = { let mut w = host.windows.lock().unwrap(); let e = w.iter_mut().find(|w| Some(&w.window) == window.as_ref()); e.map(|w| { let f = !w.relaid_out; w.relaid_out = true; f }).unwrap_or(true) };
                let flags = if first { 2 | 1 } else { 0 };
                ap::no_exception(reply)?;
                reply.write_i32(flags)?; // result flags
                reply.write_i32(1)?; // outRelayoutResult present
                host.write_relayout_result(reply, win_frame, win_display_frame)?;
                Ok(true)
            }
            "relayoutAsync" => Ok(true), // oneway
            "getWindowId" => {
                ap::no_exception(reply)?;
                reply.write(&Some(super::token::new_token("window-id")))?;
                Ok(true)
            }
            "remove" => {
                let window: Option<SIBinder> = data.read().ok().flatten();
                if let Some(ref window) = window {
                    let mut windows = host.windows.lock().unwrap();
                    if let Some(pos) = windows.iter().position(|w| &w.window == window) {
                        let removed = windows.remove(pos);
                        log::info!("window: removed window layer {} (is_child={})", removed.layer.id, removed.is_child);
                    }
                    let top_window = windows.last().map(|w| (w.window.clone(), *w.frame.lock().unwrap(), w.is_child));
                    drop(windows);
                    host.refresh_input_fd();
                    if let Some((top, win_frame, is_child)) = top_window {
                        let display_frame = host.frame();
                        let f = if is_child { win_frame } else { display_frame };
                        let _ = host.send_resized(&top, f, display_frame);
                    }
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "setInsets" | "prepareFrame" | "finishDrawing" | "updateRequestedVisibleTypes" | "updateAnimatingTypes" | "reportSystemGestureExclusionChanged" | "reportDecorViewGestureInterceptionChanged" | "reportKeepClearAreasChanged" | "setOnBackInvokedCallbackInfo" | "clearTouchableRegion" | "cancelDraw" | "pokeDrawLock" | "updateTapExcludeRegion" | "notifyImeWindowVisibilityChangedFromClient" | "onRectangleOnScreenRequested" | "setWallpaperPosition" | "setWallpaperZoomOut" | "setShouldZoomOutWallpaper" => {
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
