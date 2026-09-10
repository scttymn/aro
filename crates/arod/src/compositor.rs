//! The Wayland presenter: ARO's half of the display path. When an app posts a
//! gralloc buffer through the composer (`setTransactionState`), arod hands the
//! buffer id here; this thread maps the buffer's memfd and blits the pixels
//! into a real Hyprland window over `wl_shm`.
//!
//! One toplevel per app for now (M3). Buffers are software-rendered RGBX/RGBA,
//! copied into a shared-memory pool the compositor reads.
use crate::input_channel::{InputHub, ACTION_CANCEL, ACTION_DOWN, ACTION_MOVE, ACTION_UP};
use crate::services::allocator::Buffer;
use crate::services::surfaceflinger::{release_buffer, PostedBuffer};
use crate::services::window_session::WindowHost;
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_output, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1},
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferSlot {
    Shm(usize),
    Dmabuf(u64),
}

/// A frame to present: which buffer, and its geometry.
pub struct Frame {
    pub buffer: Arc<Buffer>,
    pub posted: PostedBuffer,
}

/// What the host says about the display the app will be shown on. Nothing in
/// here is chosen by ARO: size and refresh come from the output's current
/// mode, the scale is the user's compositor setting, and Android density is
/// defined as 160 dp per logical pixel so dp == the desktop's logical pixel.
#[derive(Clone, Debug)]
pub struct HostDisplay {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub scale: i32,
    pub refresh_mhz: i32,
}

impl HostDisplay {
    pub fn dpi(&self) -> i32 {
        160 * self.scale.max(1)
    }
    pub fn frame_interval_ns(&self) -> i64 {
        if self.refresh_mhz > 0 {
            1_000_000_000_000 / self.refresh_mhz as i64
        } else {
            16_666_667 // output advertised no refresh; 60 Hz until it does
        }
    }
    pub fn tuple(&self) -> (i32, i32, i32) {
        (self.width, self.height, self.dpi())
    }
}

/// A connected Wayland session plus the display we discovered on it.
pub struct Host {
    conn: Connection,
    pub display: HostDisplay,
}

#[derive(Default)]
struct OutputInfo {
    name: String,
    width: i32,
    height: i32,
    scale: i32,
    refresh_mhz: i32,
    done: bool,
}

#[derive(Default)]
struct Discover {
    outputs: Vec<(wl_output::WlOutput, OutputInfo)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Discover {
    fn event(d: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            if interface == "wl_output" {
                let out = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
                d.outputs.push((out, OutputInfo { scale: 1, ..Default::default() }));
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Discover {
    fn event(d: &mut Self, output: &wl_output::WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some((_, info)) = d.outputs.iter_mut().find(|(o, _)| o == output) else { return };
        match event {
            wl_output::Event::Mode { flags, width, height, refresh } => {
                let current = matches!(flags, wayland_client::WEnum::Value(f) if f.contains(wl_output::Mode::Current));
                if current || info.width == 0 {
                    info.width = width;
                    info.height = height;
                    info.refresh_mhz = refresh;
                }
            }
            wl_output::Event::Scale { factor } => info.scale = factor,
            wl_output::Event::Name { name } => info.name = name,
            wl_output::Event::Done => info.done = true,
            _ => {}
        }
    }
}

/// Connect to the session's compositor and read the display it offers.
/// Returns None when there is no Wayland display at all.
pub fn connect() -> Option<Host> {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("compositor: no Wayland display ({e})");
            return None;
        }
    };
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let mut d = Discover::default();
    conn.display().get_registry(&qh, ());
    // One roundtrip binds the outputs, the next delivers their geometry/mode/done.
    for _ in 0..2 {
        if let Err(e) = queue.roundtrip(&mut d) {
            log::warn!("compositor: output discovery failed: {e}");
            return None;
        }
    }
    let Some((_, info)) = d.outputs.iter().find(|(_, i)| i.width > 0) else {
        log::warn!("compositor: no wl_output with a mode; cannot size a display");
        return None;
    };
    let display = HostDisplay { name: info.name.clone(), width: info.width, height: info.height, scale: info.scale, refresh_mhz: info.refresh_mhz };
    log::info!("compositor: display {:?} {}x{} scale {} refresh {} mHz -> dpi {}", display.name, display.width, display.height, display.scale, display.refresh_mhz, display.dpi());
    Some(Host { conn, display })
}

#[derive(Clone)]
pub struct Presenter {
    tx: Sender<Frame>,
}

impl Presenter {
    /// Start the Wayland thread on an already-connected host. `app_id` is the
    /// app's package (so the desktop can match it to a .desktop entry) and
    /// `title` is what the window shows.
    pub fn spawn(host_conn: Host, app_id: String, title: String, input: Arc<InputHub>, host: Arc<WindowHost>) -> Option<Presenter> {
        let conn = host_conn.conn;
        let (tx, rx) = std::sync::mpsc::channel::<Frame>();
        std::thread::Builder::new()
            .name("aro-compositor".into())
            .spawn(move || {
                if let Err(e) = run(conn, rx, app_id, title, input, host) {
                    log::error!("compositor: {e}");
                }
                std::process::exit(0);
            })
            .ok()?;
        Some(Presenter { tx })
    }

    pub fn present(&self, buffer: Arc<Buffer>, posted: PostedBuffer) {
        if self.tx.send(Frame { buffer, posted: posted.clone() }).is_err() {
            release_buffer(&posted); // presenter gone: don't strand the app's buffer
        }
    }
}

const NUM_BUFFERS: usize = 3;

struct ShmSlot {
    buffer: wl_buffer::WlBuffer,
    offset: usize,
    busy: bool,
}

struct ShmPool {
    pool: wl_shm_pool::WlShmPool,
    _fd: OwnedFd,
    ptr: *mut u8,
    _frame_size: usize,
    total_len: usize,
    slots: Vec<ShmSlot>,
    next_slot: usize,
}

impl ShmPool {
    fn acquire_free_slot(&mut self) -> Option<usize> {
        for i in 0..self.slots.len() {
            let idx = (self.next_slot + i) % self.slots.len();
            if !self.slots[idx].busy {
                self.next_slot = (idx + 1) % self.slots.len();
                return Some(idx);
            }
        }
        None
    }
}

impl Drop for ShmPool {
    fn drop(&mut self) {
        for slot in &self.slots {
            slot.buffer.destroy();
        }
        unsafe { libc::munmap(self.ptr as *mut _, self.total_len) };
        self.pool.destroy();
    }
}

struct OverlayData {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    stride: u32,
    pixels: Vec<u8>,
}

struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    formats: Vec<wl_shm::Format>,
    surface: Option<wl_surface::WlSurface>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    configured: bool,
    closed: bool,
    title: String,
    // Reusable multi-buffer shm pool.
    shm_pool: Option<ShmPool>,
    base_canvas: Vec<u8>,
    surface_view_canvas: Option<Vec<u8>>,
    base_w: u32,
    base_h: u32,
    base_stride: u32,
    overlay: Option<OverlayData>,
    needs_present: bool,
    last_size: (u32, u32),
    // Input + window host (frame size follows the toplevel's configure).
    input: Arc<InputHub>,
    host: Arc<WindowHost>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    ptr_x: f32,
    ptr_y: f32,
    ptr_down: bool,
    over_surface: bool,
    down_layer_id: Option<i32>,
    active_base_layer_id: Option<i32>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    dmabuf_buffers: HashMap<u64, wl_buffer::WlBuffer>,
    in_flight_dmabuf: HashMap<u64, PostedBuffer>,
}

impl App {
    fn send_pointer_motion(&mut self, action: i32, screen_x: f32, screen_y: f32) -> bool {
        let windows = self.host.windows.lock().unwrap();
        if action == ACTION_DOWN {
            if let Some(top) = windows.last() {
                self.down_layer_id = Some(top.layer.id);
                let (local_x, local_y) = if top.is_child {
                    let (px, py) = *top.pos.lock().unwrap();
                    (screen_x - px as f32, screen_y - py as f32)
                } else {
                    (screen_x, screen_y)
                };
                if let Ok(dup) = top.input_tx.try_clone() {
                    *self.input.fd.lock().unwrap() = Some(dup);
                }
                drop(windows);
                return self.input.send_motion(action, local_x, local_y, screen_x, screen_y);
            }
            return false;
        }

        if let Some(target_id) = self.down_layer_id {
            if let Some(w) = windows.iter().find(|w| w.layer.id == target_id) {
                let (local_x, local_y) = if w.is_child {
                    let (px, py) = *w.pos.lock().unwrap();
                    (screen_x - px as f32, screen_y - py as f32)
                } else {
                    (screen_x, screen_y)
                };
                if let Ok(dup) = w.input_tx.try_clone() {
                    *self.input.fd.lock().unwrap() = Some(dup);
                }
                if action == ACTION_UP || action == ACTION_CANCEL {
                    self.down_layer_id = None;
                }
                drop(windows);
                return self.input.send_motion(action, local_x, local_y, screen_x, screen_y);
            } else {
                log::info!("compositor: gesture target layer {target_id} was removed; ignoring action={action}");
                if action == ACTION_UP || action == ACTION_CANCEL {
                    self.down_layer_id = None;
                }
                return true;
            }
        }

        let (local_x, local_y) = if let Some(top) = windows.last() {
            if top.is_child {
                let (px, py) = *top.pos.lock().unwrap();
                (screen_x - px as f32, screen_y - py as f32)
            } else {
                (screen_x, screen_y)
            }
        } else {
            (screen_x, screen_y)
        };
        drop(windows);
        self.input.send_motion(action, local_x, local_y, screen_x, screen_y)
    }

    fn recreate_pool(&mut self, qh: &QueueHandle<App>, w: u32, h: u32, stride: u32, size: usize, format: i32) {
        self.shm_pool = None;
        let Some((wl_format, _)) = shm_format(format, &self.formats) else {
            log::warn!("compositor: unsupported format {format}");
            return;
        };
        let shm = self.shm.clone().unwrap();
        let total_len = size * NUM_BUFFERS;
        let raw = unsafe { libc::memfd_create(c"aro-wl".as_ptr(), libc::MFD_CLOEXEC) };
        if raw < 0 {
            log::error!("compositor: memfd_create: {}", std::io::Error::last_os_error());
            return;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), total_len as i64) } != 0 {
            log::error!("compositor: ftruncate: {}", std::io::Error::last_os_error());
            return;
        }
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), total_len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            log::error!("compositor: mmap: {}", std::io::Error::last_os_error());
            return;
        }
        let pool = shm.create_pool(fd.as_fd(), total_len as i32, qh, ());
        let mut slots = Vec::with_capacity(NUM_BUFFERS);
        for i in 0..NUM_BUFFERS {
            let offset = i * size;
            let wl_buffer = pool.create_buffer(offset as i32, w as i32, h as i32, (stride * 4) as i32, wl_format, qh, BufferSlot::Shm(i));
            slots.push(ShmSlot {
                buffer: wl_buffer,
                offset,
                busy: false,
            });
        }
        self.shm_pool = Some(ShmPool {
            pool,
            _fd: fd,
            ptr: ptr as *mut u8,
            _frame_size: size,
            total_len,
            slots,
            next_slot: 0,
        });
        self.last_size = (w, h);
    }
}

fn run(conn: Connection, rx: Receiver<Frame>, app_id: String, title: String, input: Arc<InputHub>, host: Arc<WindowHost>) -> anyhow::Result<()> {
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let display = conn.display();
    display.get_registry(&qh, ());

    let mut app = App {
        compositor: None,
        shm: None,
        wm_base: None,
        formats: Vec::new(),
        surface: None,
        xdg_surface: None,
        toplevel: None,
        configured: false,
        closed: false,
        title,
        shm_pool: None,
        base_canvas: Vec::new(),
        surface_view_canvas: None,
        base_w: 0,
        base_h: 0,
        base_stride: 0,
        overlay: None,
        needs_present: false,
        last_size: (0, 0),
        input,
        host,
        seat: None,
        pointer: None,
        ptr_x: 0.0,
        ptr_y: 0.0,
        ptr_down: false,
        over_surface: false,
        down_layer_id: None,
        active_base_layer_id: None,
        dmabuf: None,
        dmabuf_buffers: HashMap::new(),
        in_flight_dmabuf: HashMap::new(),
    };

    // Bind globals.
    queue.roundtrip(&mut app)?;
    let (Some(compositor), Some(wm_base)) = (app.compositor.clone(), app.wm_base.clone()) else {
        anyhow::bail!("compositor missing wl_compositor/xdg_wm_base");
    };
    if app.shm.is_none() {
        anyhow::bail!("compositor missing wl_shm");
    }

    // Create the toplevel.
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title(app.title.clone());
    toplevel.set_app_id(app_id);
    surface.set_buffer_scale(app.host.scale);
    surface.commit();
    app.surface = Some(surface);
    app.xdg_surface = Some(xdg_surface);
    app.toplevel = Some(toplevel);

    // Wait for the first configure so we may attach buffers.
    while !app.configured && !app.closed {
        queue.blocking_dispatch(&mut app)?;
    }

    let mut presents = 0u32;
    let mut test_taps: Vec<(f32, f32)> = Vec::new();
    let mut tap_step: Option<(usize, std::time::Instant, bool)> = None;
    loop {
        if app.closed {
            log::info!("compositor: window closed, terminating app");
            std::process::exit(0);
        }
        // Drain posts; process frames.
        while let Ok(frame) = rx.try_recv() {
            let overlay_info = {
                let windows = app.host.windows.lock().unwrap();
                let (hw, hh) = *app.host.size.lock().unwrap();
                let bw = frame.buffer.desc.width as i32;
                let bh = frame.buffer.desc.height as i32;
                if frame.buffer.desc.format == 1 {
                    windows.iter().rev().find(|w| {
                        if !w.is_child { return false; }
                        let (sw, sh) = *w.size.lock().unwrap();
                        let is_popup = sw < hw - 100 || sh < hh - 100;
                        if is_popup {
                            bw <= sw + 256 && bh <= sh + 256
                        } else {
                            bw <= hw + 256 && bh <= hh + 256
                        }
                    }).map(|w| (*w.pos.lock().unwrap(), *w.size.lock().unwrap()))
                } else {
                    None
                }
            };

            let buf = &frame.buffer;
            let (w, h, stride, size) = (buf.desc.width, buf.desc.height, buf.stride, buf.size as usize);

            // Wait for GPU rendering to complete before reading buffer via CPU
            if let Some(ref fence) = frame.posted.acquire_fence {
                let mut pfd = libc::pollfd {
                    fd: fence.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ret = unsafe { libc::poll(&mut pfd, 1, 3000) };
                log::info!("compositor: acquire_fence wait on fd {} -> {ret} (revents={:#x})", fence.as_raw_fd(), pfd.revents);
            }

            #[repr(C)]
            struct DmaBufSync {
                flags: u64,
            }
            const DMA_BUF_SYNC_READ: u64 = 1;
            const DMA_BUF_SYNC_START: u64 = 0;
            const DMA_BUF_SYNC_END: u64 = 4;
            const DMA_BUF_IOCTL_SYNC: libc::c_ulong = 0x40086200;

            let sync_start = DmaBufSync { flags: DMA_BUF_SYNC_READ | DMA_BUF_SYNC_START };
            unsafe { libc::ioctl(buf.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC, &sync_start) };

            let src = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, buf.fd.as_raw_fd(), 0) };
            if src == libc::MAP_FAILED {
                log::error!("compositor: mmap gralloc buffer failed: {}", std::io::Error::last_os_error());
                release_buffer(&frame.posted);
                continue;
            }

            let tag = crate::services::surfaceflinger::extract_window_tag(&buf.desc.name);

            let is_overlay = if let Some(((win_x, win_y), (win_w, win_h))) = overlay_info {
                let pad_x = ((w as i32 - win_w) / 2).max(0);
                let pad_y = ((h as i32 - win_h) / 2).max(0);
                let draw_x = win_x - pad_x;
                let draw_y = win_y - pad_y;

                let mut pixels = vec![0u8; size];
                unsafe {
                    std::ptr::copy_nonoverlapping(src as *const u8, pixels.as_mut_ptr(), size);
                    libc::munmap(src, size);
                }
                let sync_end = DmaBufSync { flags: DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END };
                unsafe { libc::ioctl(buf.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC, &sync_end) };
                log::info!("compositor: overlay buffer {} {:?} ({}x{}) at ({draw_x},{draw_y}) pad=({pad_x},{pad_y})", buf.id, buf.desc.name, w, h);
                app.overlay = Some(OverlayData {
                    x: draw_x,
                    y: draw_y,
                    width: w,
                    height: h,
                    stride,
                    pixels,
                });
                release_buffer(&frame.posted);
                app.needs_present = true;
                true
            } else {
                false
            };

            if !is_overlay {
                let windows = app.host.windows.lock().unwrap();
                let base_windows: Vec<_> = windows.iter().filter(|w| !w.is_child).collect();
                let matching_win = base_windows.iter().find(|w| {
                    w.tag.lock().unwrap().as_deref() == Some(&tag)
                }).or_else(|| {
                    base_windows.iter().find(|w| {
                        w.tag.lock().unwrap().as_ref().map(|wt| wt.contains(&tag) || tag.contains(wt.as_str())).unwrap_or(false)
                    })
                }).or_else(|| {
                    if base_windows.len() == 1 {
                        base_windows.first()
                    } else {
                        None
                    }
                });

                if matching_win.is_none() && frame.buffer.desc.format == 1 {
                    log::info!("compositor: dropping orphaned format 1 buffer {} {:?} ({}x{}) after child dismissed", buf.id, buf.desc.name, w, h);
                    unsafe { libc::munmap(src, size) };
                    release_buffer(&frame.posted);
                    continue;
                }

                let is_surface_view = buf.desc.name.contains("SurfaceView");
                let is_top_base = base_windows.last().map(|top| {
                    matching_win.map(|mw| Arc::ptr_eq(&mw.layer, &top.layer)).unwrap_or(false)
                }).unwrap_or(false);

                let can_zero_copy = buf.is_gbm
                    && app.dmabuf.is_some()
                    && app.overlay.is_none()
                    && is_top_base
                    && !is_surface_view
                    && std::env::var_os("ARO_DISABLE_ZERO_COPY").is_none()
                    && (std::env::var_os("ARO_FORCE_ZERO_COPY").is_some() || !is_cross_gpu_system());

                if can_zero_copy {
                    unsafe { libc::munmap(src, size) };
                    drop(windows);

                    let wl_buf = if let Some(wb) = app.dmabuf_buffers.get(&buf.id) {
                        wb.clone()
                    } else {
                        let dmabuf = app.dmabuf.as_ref().unwrap();
                        let params = dmabuf.create_params(&qh, ());
                        let drm_fmt = match buf.desc.format {
                            1 | 2 => 0x34324241, // ABGR8888 ('AB24')
                            5 => 0x34325241, // ARGB8888 ('AR24')
                            _ => 0x34324241,
                        };
                        let mod_hi = (buf.modifier >> 32) as u32;
                        let mod_lo = (buf.modifier & 0xffff_ffff) as u32;
                        log::info!("compositor: dmabuf params.add fmt={drm_fmt:#x} mod={:#x} ({mod_hi:#x}, {mod_lo:#x})", buf.modifier);
                        params.add(
                            buf.fd.as_fd(),
                            0,
                            0,
                            buf.stride * 4,
                            mod_hi,
                            mod_lo,
                        );
                        let wb = params.create_immed(
                            w as i32,
                            h as i32,
                            drm_fmt,
                            zwp_linux_buffer_params_v1::Flags::empty(),
                            &qh,
                            BufferSlot::Dmabuf(frame.posted.gb_id),
                        );
                        params.destroy();
                        app.dmabuf_buffers.insert(buf.id, wb.clone());
                        wb
                    };

                    let surface = app.surface.as_ref().unwrap();
                    if let Some(comp) = &app.compositor {
                        let region = comp.create_region(&qh, ());
                        region.add(0, 0, w as i32, h as i32);
                        surface.set_opaque_region(Some(&region));
                        region.destroy();
                    }
                    surface.attach(Some(&wl_buf), 0, 0);
                    surface.damage_buffer(0, 0, w as i32, h as i32);
                    surface.commit();
                    app.in_flight_dmabuf.insert(frame.posted.gb_id, frame.posted.clone());
                    presents += 1;
                    log::info!("compositor: zero-copy presented dmabuf {} ({w}x{h}) gb={:#x} frame={}", buf.id, frame.posted.gb_id, frame.posted.frame_number);

                    if presents == 1 && std::env::var_os("ARO_TEST_TAP").is_some() {
                        let val = std::env::var("ARO_TEST_TAP").unwrap_or_default();
                        for part in val.split(';') {
                            let part = part.trim();
                            if part.is_empty() { continue; }
                            if let Some((xs, ys)) = part.split_once(',') {
                                if let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>()) {
                                    test_taps.push((x, y));
                                }
                            }
                        }
                        if !test_taps.is_empty() {
                            tap_step = Some((0, std::time::Instant::now(), false));
                        }
                    }
                    continue;
                }

                let mut pixels = vec![0u8; size];
                unsafe {
                    std::ptr::copy_nonoverlapping(src as *const u8, pixels.as_mut_ptr(), size);
                    libc::munmap(src, size);
                }
                let sync_end = DmaBufSync { flags: DMA_BUF_SYNC_READ | DMA_BUF_SYNC_END };
                unsafe { libc::ioctl(buf.fd.as_raw_fd(), DMA_BUF_IOCTL_SYNC, &sync_end) };
                release_buffer(&frame.posted);

                if let Some(target) = matching_win {
                    *target.canvas.lock().unwrap() = Some(crate::services::window_session::WindowCanvas {
                        width: w,
                        height: h,
                        stride,
                        pixels: pixels.clone(),
                    });
                }

                if is_top_base {
                    if is_surface_view {
                        app.surface_view_canvas = Some(pixels);
                    } else {
                        app.base_canvas = pixels;
                    }
                    app.base_w = w;
                    app.base_h = h;
                    app.base_stride = stride;
                    app.needs_present = true;
                    if let Some(top) = base_windows.last() {
                        app.active_base_layer_id = Some(top.layer.id);
                    }
                    log::info!("compositor: base buffer {} {:?} ({}x{}) presented to display (sv={is_surface_view})", buf.id, buf.desc.name, w, h);
                } else if matching_win.is_some() {
                    log::info!("compositor: base buffer {} {:?} ({}x{}) belongs to background window; cached without presenting", buf.id, buf.desc.name, w, h);
                } else {
                    log::info!("compositor: base buffer {} {:?} ({}x{}) has no matching window; dropped", buf.id, buf.desc.name, w, h);
                }
            }
        }

        // Check if top base window changed (e.g. after a window was closed/removed)
        {
            let windows = app.host.windows.lock().unwrap();
            let top_base = windows.iter().rev().find(|w| !w.is_child);
            let top_layer_id = top_base.map(|w| w.layer.id);
            if top_layer_id != app.active_base_layer_id {
                if let Some(top_win) = top_base {
                    if let Some(canvas) = top_win.canvas.lock().unwrap().as_ref() {
                        log::info!("compositor: active base window changed to layer {}, restoring canvas ({}x{})", top_win.layer.id, canvas.width, canvas.height);
                        app.base_canvas = canvas.pixels.clone();
                        app.surface_view_canvas = None;
                        app.base_w = canvas.width;
                        app.base_h = canvas.height;
                        app.base_stride = canvas.stride;
                        app.active_base_layer_id = Some(top_win.layer.id);
                        app.needs_present = true;
                    }
                }
            }
        }

        // Check if child window was dismissed
        if app.overlay.is_some() {
            let has_child = app.host.windows.lock().unwrap().iter().any(|w| w.is_child);
            if !has_child {
                log::info!("compositor: child window dismissed, clearing overlay");
                app.overlay = None;
                app.needs_present = true;
            }
        }

        if app.needs_present && (!app.base_canvas.is_empty() || app.surface_view_canvas.is_some()) {
            let (w, h, stride) = (app.base_w, app.base_h, app.base_stride);
            let size = (stride * h * 4) as usize;
            if app.last_size != (w, h) || app.shm_pool.is_none() {
                app.recreate_pool(&qh, w, h, stride, size, 2);
            }
            if let Some(slot_idx) = app.shm_pool.as_mut().and_then(|p| p.acquire_free_slot()) {
                present_composite(&mut app, &qh, slot_idx);
                app.needs_present = false;
                presents += 1;
                // Debug: inject taps after the first frame if configured.
                if presents == 1 && std::env::var_os("ARO_TEST_TAP").is_some() {
                    let val = std::env::var("ARO_TEST_TAP").unwrap_or_default();
                    for part in val.split(';') {
                        let part = part.trim();
                        if part.is_empty() { continue; }
                        if let Some((xs, ys)) = part.split_once(',') {
                            if let (Ok(x), Ok(y)) = (xs.trim().parse::<f32>(), ys.trim().parse::<f32>()) {
                                test_taps.push((x, y));
                            }
                        }
                    }
                    if !test_taps.is_empty() {
                        tap_step = Some((0, std::time::Instant::now(), false));
                    }
                }
            }
        }
        if let Some((idx, at, down_sent)) = tap_step {
            let (cx, cy) = test_taps[idx];
            if !down_sent && at.elapsed() >= std::time::Duration::from_millis(2000) {
                log::info!("compositor: test tap [{idx}] DOWN at ({cx:.0},{cy:.0})");
                app.send_pointer_motion(ACTION_DOWN, cx, cy);
                tap_step = Some((idx, at, true));
            } else if down_sent && at.elapsed() >= std::time::Duration::from_millis(2100) {
                log::info!("compositor: test tap [{idx}] UP at ({cx:.0},{cy:.0})");
                app.send_pointer_motion(ACTION_UP, cx, cy);
                if idx + 1 < test_taps.len() {
                    tap_step = Some((idx + 1, std::time::Instant::now(), false));
                } else {
                    tap_step = None;
                }
            }
        }
        app.input.drain_finished();
        conn.flush()?;
        // Block briefly on Wayland events, but wake often to check the channel.
        queue.dispatch_pending(&mut app)?;
        if let Some(guard) = conn.prepare_read() {
            let fd = guard.connection_fd();
            let mut pfd = [libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let timeout = if app.needs_present { 1 } else { 4 };
            let n = unsafe { libc::poll(pfd.as_mut_ptr(), 1, timeout) };
            if n > 0 && (pfd[0].revents & libc::POLLIN) != 0 {
                let _ = guard.read();
            } else {
                drop(guard);
            }
            queue.dispatch_pending(&mut app)?;
        } else {
            queue.dispatch_pending(&mut app)?;
        }
    }
}

fn is_cross_gpu_system() -> bool {
    let mut vendors = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/dev/dri") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("renderD") {
                let sys_vendor = format!("/sys/class/drm/{}/device/vendor", name);
                if let Ok(s) = std::fs::read_to_string(&sys_vendor) {
                    if let Ok(v) = u32::from_str_radix(s.trim().trim_start_matches("0x"), 16) {
                        if !vendors.contains(&v) {
                            vendors.push(v);
                        }
                    }
                }
            }
        }
    }
    vendors.len() > 1
}

fn shm_format(android_format: i32, supported: &[wl_shm::Format]) -> Option<(wl_shm::Format, bool)> {
    // Android RGBX/RGBA_8888 memory order is R,G,B,X. Wayland XBGR/ABGR8888 match
    // byte-for-byte; XRGB/ARGB need an R<->B swizzle.
    let has = |f: wl_shm::Format| supported.contains(&f);
    match android_format {
        1 | 2 => {
            // RGBA_8888 / RGBX_8888
            if has(wl_shm::Format::Xbgr8888) {
                Some((wl_shm::Format::Xbgr8888, false))
            } else {
                Some((wl_shm::Format::Xrgb8888, true))
            }
        }
        5 => {
            // BGRA_8888 already B,G,R,A
            Some((wl_shm::Format::Argb8888, false))
        }
        _ => None,
    }
}

fn present_composite(app: &mut App, qh: &QueueHandle<App>, slot_idx: usize) {
    let (w, h, stride) = (app.base_w, app.base_h, app.base_stride);
    let size = (stride * h * 4) as usize;
    let Some((_, swizzle)) = shm_format(2, &app.formats) else {
        log::warn!("compositor: unsupported format for base window");
        return;
    };
    let pool = app.shm_pool.as_mut().unwrap();
    let slot = &mut pool.slots[slot_idx];
    let dst = unsafe { pool.ptr.add(slot.offset) };

    unsafe {
        if let Some(ref sv) = app.surface_view_canvas {
            let sv_size = sv.len().min(size);
            std::ptr::copy_nonoverlapping(sv.as_ptr(), dst, sv_size);
            for i in (3..size).step_by(4) {
                *dst.add(i) = 255;
            }

            // Blend non-zero VRI pixels on top of SurfaceView
            if !app.base_canvas.is_empty() && app.base_canvas.len() == size {
                for i in (0..size).step_by(4) {
                    let a = app.base_canvas[i + 3];
                    if a > 0 {
                        let r = app.base_canvas[i];
                        let g = app.base_canvas[i + 1];
                        let b = app.base_canvas[i + 2];
                        if a == 255 {
                            *dst.add(i) = r;
                            *dst.add(i + 1) = g;
                            *dst.add(i + 2) = b;
                        } else {
                            let inv_a = 255 - a as u32;
                            let dr = *dst.add(i) as u32;
                            let dg = *dst.add(i + 1) as u32;
                            let db = *dst.add(i + 2) as u32;
                            *dst.add(i) = (r as u32 + (dr * inv_a + 127) / 255).min(255) as u8;
                            *dst.add(i + 1) = (g as u32 + (dg * inv_a + 127) / 255).min(255) as u8;
                            *dst.add(i + 2) = (b as u32 + (db * inv_a + 127) / 255).min(255) as u8;
                        }
                    }
                }
            }
        } else if !app.base_canvas.is_empty() {
            let b_size = app.base_canvas.len().min(size);
            std::ptr::copy_nonoverlapping(app.base_canvas.as_ptr(), dst, b_size);
            for i in (3..size).step_by(4) {
                *dst.add(i) = 255;
            }
        }

        // If overlay is present, blend it on top of dst
        if let Some(ref ov) = app.overlay {
            let ow = ov.width as i32;
            let oh = ov.height as i32;
            let o_stride = ov.stride as i32;
            let b_w = w as i32;
            let b_h = h as i32;
            let b_stride = stride as i32;

            let oy_start = (-ov.y).max(0) as usize;
            let oy_end = (b_h - ov.y).clamp(0, oh) as usize;
            let ox_start = (-ov.x).max(0) as usize;
            let ox_end = (b_w - ov.x).clamp(0, ow) as usize;

            for oy in oy_start..oy_end {
                let sy = ov.y + oy as i32;
                let o_row_off = (oy as i32 * o_stride * 4) as usize;
                let b_row_off = (sy * b_stride * 4) as usize;

                for ox in ox_start..ox_end {
                    let sx = ov.x + ox as i32;
                    let o_px = o_row_off + ox * 4;
                    let sa = ov.pixels[o_px + 3];
                    if sa == 0 {
                        continue;
                    }
                    let sr = ov.pixels[o_px];
                    let sg = ov.pixels[o_px + 1];
                    let sb = ov.pixels[o_px + 2];

                    let b_px = b_row_off + sx as usize * 4;
                    let d = dst.add(b_px);

                    if sa == 255 {
                        *d = sr;
                        *d.add(1) = sg;
                        *d.add(2) = sb;
                        *d.add(3) = 255;
                    } else {
                        let inv_a = 255 - sa as u32;
                        let dr = *d as u32;
                        let dg = *d.add(1) as u32;
                        let db = *d.add(2) as u32;

                        *d = (sr as u32 + (dr * inv_a + 127) / 255).min(255) as u8;
                        *d.add(1) = (sg as u32 + (dg * inv_a + 127) / 255).min(255) as u8;
                        *d.add(2) = (sb as u32 + (db * inv_a + 127) / 255).min(255) as u8;
                        *d.add(3) = 255;
                    }
                }
            }
        }

        // Apply swizzle if required (Wayland XRGB instead of XBGR)
        if swizzle {
            for i in (0..size).step_by(4) {
                let r = *dst.add(i);
                let b = *dst.add(i + 2);
                *dst.add(i) = b;
                *dst.add(i + 2) = r;
            }
        }
    }

    if std::env::var_os("ARO_DUMP_FRAMES").is_some() {
        static DUMPED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = DUMPED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < 15 {
            let path = format!("/tmp/aro_frames/frame-{n:02}-slot{slot_idx}.ppm");
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            unsafe {
                for y in 0..h {
                    let row_start = (y * stride * 4) as usize;
                    for x in 0..w {
                        let i = row_start + (x * 4) as usize;
                        let (b0, b1, b2) = (*dst.add(i), *dst.add(i + 1), *dst.add(i + 2));
                        if swizzle { out.extend_from_slice(&[b2, b1, b0]); } else { out.extend_from_slice(&[b0, b1, b2]); }
                    }
                }
            }
            let _ = std::fs::write(&path, out);
        }
    }

    let surface = app.surface.as_ref().unwrap();
    if let Some(comp) = &app.compositor {
        let region = comp.create_region(qh, ());
        region.add(0, 0, w as i32, h as i32);
        surface.set_opaque_region(Some(&region));
        region.destroy();
    }
    surface.attach(Some(&slot.buffer), 0, 0);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    slot.busy = true;
    log::debug!("compositor: presented composite on slot {slot_idx} ({w}x{h}) overlay={}", app.overlay.is_some());
}

// --- Global binding ---
impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(app: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            log::info!("wayland: global {} v{}", interface, version);
            match interface.as_str() {
                "wl_compositor" => {
                    app.compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(name, version.min(5), qh, ()));
                }
                "wl_shm" => {
                    app.shm = Some(registry.bind::<wl_shm::WlShm, _, _>(name, version.min(1), qh, ()));
                }
                "xdg_wm_base" => {
                    app.wm_base = Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(name, version.min(3), qh, ()));
                }
                "wl_seat" => {
                    app.seat = Some(registry.bind::<wl_seat::WlSeat, _, _>(name, version.min(5), qh, ()));
                }
                "zwp_linux_dmabuf_v1" => {
                    app.dmabuf = Some(registry.bind::<ZwpLinuxDmabufV1, _, _>(name, version.min(4), qh, ()));
                    log::info!("compositor: bound zwp_linux_dmabuf_v1 v{}", version.min(4));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_shm::WlShm, ()> for App {
    fn event(app: &mut Self, _: &wl_shm::WlShm, event: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_shm::Event::Format { format } = event {
            if let wayland_client::WEnum::Value(f) = format {
                app.formats.push(f);
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(_: &mut Self, wm_base: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(app: &mut Self, xdg_surface: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            app.configured = true;
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for App {
    fn event(app: &mut Self, _: &xdg_toplevel::XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg_toplevel::Event::Close => app.closed = true,
            xdg_toplevel::Event::Configure { width, height, .. } => {
                // 0x0 means "you choose"; otherwise the desktop tiled/resized us.
                if width > 0 && height > 0 {
                    app.host.resize(width, height);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferSlot> for App {
    fn event(app: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, &slot: &BufferSlot, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            match slot {
                BufferSlot::Shm(slot_idx) => {
                    if let Some(pool) = &mut app.shm_pool {
                        if let Some(slot) = pool.slots.get_mut(slot_idx) {
                            slot.busy = false;
                        }
                    }
                }
                BufferSlot::Dmabuf(gb_id) => {
                    log::debug!("compositor: wayland released dmabuf gb={gb_id:#x}");
                    if let Some(pb) = app.in_flight_dmabuf.remove(&gb_id) {
                        release_buffer(&pb);
                    }
                }
            }
        }
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for App {
    fn event(_: &mut Self, _: &ZwpLinuxDmabufV1, event: zwp_linux_dmabuf_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwp_linux_dmabuf_v1::Event::Format { format } => {
                let fourcc = [format as u8, (format >> 8) as u8, (format >> 16) as u8, (format >> 24) as u8];
                let s = String::from_utf8_lossy(&fourcc);
                log::info!("dmabuf: format {format:#x} ({s})");
            }
            zwp_linux_dmabuf_v1::Event::Modifier { format, modifier_hi, modifier_lo } => {
                let fourcc = [format as u8, (format >> 8) as u8, (format >> 16) as u8, (format >> 24) as u8];
                let s = String::from_utf8_lossy(&fourcc);
                let modif = ((modifier_hi as u64) << 32) | (modifier_lo as u64);
                log::info!("dmabuf: modifier format {format:#x} ({s}) mod={modif:#x}");
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for App {
    fn event(_: &mut Self, _: &ZwpLinuxBufferParamsV1, event: zwp_linux_buffer_params_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { .. } => {
                log::info!("dmabuf: buffer created");
            }
            zwp_linux_buffer_params_v1::Event::Failed => {
                log::error!("dmabuf: buffer creation FAILED!");
            }
            _ => {}
        }
    }
}


impl Dispatch<wl_seat::WlSeat, ()> for App {
    fn event(app: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            if let wayland_client::WEnum::Value(caps) = capabilities {
                if caps.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
                    app.pointer = Some(seat.get_pointer(qh, ()));
                }
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for App {
    fn event(app: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use wl_pointer::Event;
        const BTN_LEFT: u32 = 0x110;
        match event {
            Event::Enter { surface_x, surface_y, .. } => {
                app.over_surface = true;
                app.ptr_x = surface_x as f32 * app.host.scale as f32;
                app.ptr_y = surface_y as f32 * app.host.scale as f32;
            }
            Event::Leave { .. } => {
                app.over_surface = false;
                if app.ptr_down {
                    app.ptr_down = false;
                    app.send_pointer_motion(ACTION_CANCEL, app.ptr_x, app.ptr_y);
                }
            }
            Event::Motion { surface_x, surface_y, .. } => {
                app.ptr_x = surface_x as f32 * app.host.scale as f32;
                app.ptr_y = surface_y as f32 * app.host.scale as f32;
                if app.ptr_down {
                    app.send_pointer_motion(ACTION_MOVE, app.ptr_x, app.ptr_y);
                }
            }
            Event::Button { button, state, .. } => {
                if button != BTN_LEFT {
                    return;
                }
                match state {
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        app.ptr_down = true;
                        if !app.send_pointer_motion(ACTION_DOWN, app.ptr_x, app.ptr_y) {
                            app.host.refresh_input_fd();
                            app.send_pointer_motion(ACTION_DOWN, app.ptr_x, app.ptr_y);
                        }
                    }
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Released) => {
                        app.ptr_down = false;
                        app.send_pointer_motion(ACTION_UP, app.ptr_x, app.ptr_y);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_compositor::WlCompositor, ()> for App {
    fn event(_: &mut Self, _: &wl_compositor::WlCompositor, _: wl_compositor::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_surface::WlSurface, ()> for App {
    fn event(_: &mut Self, _: &wl_surface::WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for App {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_region::WlRegion, ()> for App {
    fn event(_: &mut Self, _: &wl_region::WlRegion, _: wl_region::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
