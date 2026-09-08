//! The Wayland presenter: ARO's half of the display path. When an app posts a
//! gralloc buffer through the composer (`setTransactionState`), arod hands the
//! buffer id here; this thread maps the buffer's memfd and blits the pixels
//! into a real Hyprland window over `wl_shm`.
//!
//! One toplevel per app for now (M3). Buffers are software-rendered RGBX/RGBA,
//! copied into a shared-memory pool the compositor reads.
use crate::input_channel::{InputHub, ACTION_DOWN, ACTION_MOVE, ACTION_UP};
use crate::services::allocator::Buffer;
use crate::services::surfaceflinger::{release_buffer, PostedBuffer};
use crate::services::window_session::WindowHost;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

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
    // Reusable shm pool.
    pool: Option<(wl_shm_pool::WlShmPool, OwnedFd, *mut u8, usize)>,
    wl_buffer: Option<wl_buffer::WlBuffer>,
    buffer_busy: bool,
    pending: Option<Frame>,
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
        pool: None,
        wl_buffer: None,
        buffer_busy: false,
        pending: None,
        last_size: (0, 0),
        input,
        host,
        seat: None,
        pointer: None,
        ptr_x: 0.0,
        ptr_y: 0.0,
        ptr_down: false,
        over_surface: false,
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
    let mut test_tap: Option<(f32, f32, std::time::Instant, bool)> = None;
    loop {
        if app.closed {
            log::info!("compositor: window closed");
            return Ok(());
        }
        // Drain posts; keep only the freshest buffer.
        while let Ok(frame) = rx.try_recv() {
            // Only the freshest frame is shown; a superseded one goes straight back.
            if let Some(old) = app.pending.replace(frame) {
                release_buffer(&old.posted);
            }
        }
        if app.pending.is_some() && !app.buffer_busy {
            if let Some(frame) = app.pending.take() {
                let buf = frame.buffer.clone();
                present_buffer(&mut app, &qh, &buf);
                // The pixels are in our shm pool now; the app may reuse its buffer.
                release_buffer(&frame.posted);
                presents += 1;
                // Debug: inject a tap at the window centre a moment after the
                // first frame, to exercise input without a desktop pointer.
                if presents == 1 && std::env::var_os("ARO_TEST_TAP").is_some() {
                    let (cx, cy) = (buf.desc.width as f32 / 2.0, buf.desc.height as f32 / 2.0);
                    test_tap = Some((cx, cy, std::time::Instant::now(), false));
                }
            }
        }
        if let Some((cx, cy, at, down_sent)) = test_tap {
            if !down_sent && at.elapsed() >= std::time::Duration::from_millis(2000) {
                log::info!("compositor: test tap DOWN at ({cx:.0},{cy:.0})");
                app.input.send_motion(ACTION_DOWN, cx, cy);
                test_tap = Some((cx, cy, at, true));
            } else if down_sent && at.elapsed() >= std::time::Duration::from_millis(2300) {
                log::info!("compositor: test tap UP at ({cx:.0},{cy:.0})");
                app.input.send_motion(ACTION_UP, cx, cy);
                test_tap = None;
            }
        }
        app.input.drain_finished();
        conn.flush()?;
        // Block briefly on Wayland events, but wake often to check the channel.
        queue.dispatch_pending(&mut app)?;
        if let Some(guard) = conn.prepare_read() {
            let fd = guard.connection_fd();
            let mut pfd = [libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
            let n = unsafe { libc::poll(pfd.as_mut_ptr(), 1, 8) };
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

fn present_buffer(app: &mut App, qh: &QueueHandle<App>, buf: &Arc<Buffer>) {
    let (w, h, stride, size) = (buf.desc.width, buf.desc.height, buf.stride, buf.size as usize);
    let Some((format, swizzle)) = shm_format(buf.desc.format, &app.formats) else {
        log::warn!("compositor: unsupported format {}", buf.desc.format);
        return;
    };
    let shm = app.shm.clone().unwrap();

    // (Re)create the pool and wl_buffer if the geometry changed.
    if app.last_size != (w, h) || app.pool.is_none() {
        if let Some(b) = app.wl_buffer.take() {
            b.destroy();
        }
        if let Some((pool, _, ptr, len)) = app.pool.take() {
            unsafe { libc::munmap(ptr as *mut _, len) };
            pool.destroy();
        }
        let raw = unsafe { libc::memfd_create(c"aro-wl".as_ptr(), libc::MFD_CLOEXEC) };
        if raw < 0 {
            log::error!("compositor: memfd_create: {}", std::io::Error::last_os_error());
            return;
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::ftruncate(fd.as_raw_fd(), size as i64) } != 0 {
            log::error!("compositor: ftruncate: {}", std::io::Error::last_os_error());
            return;
        }
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd.as_raw_fd(), 0) };
        if ptr == libc::MAP_FAILED {
            log::error!("compositor: mmap: {}", std::io::Error::last_os_error());
            return;
        }
        let pool = shm.create_pool(fd.as_fd(), size as i32, qh, ());
        let wl_buffer = pool.create_buffer(0, w as i32, h as i32, (stride * 4) as i32, format, qh, ());
        app.pool = Some((pool, fd, ptr as *mut u8, size));
        app.wl_buffer = Some(wl_buffer);
        app.last_size = (w, h);
    }

    // Map the gralloc buffer and copy into the shm pool.
    let src = unsafe { libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, buf.fd.as_raw_fd(), 0) };
    if src == libc::MAP_FAILED {
        log::error!("compositor: map gralloc buffer: {}", std::io::Error::last_os_error());
        return;
    }
    let (_, _, dst, _) = app.pool.as_ref().unwrap();
    unsafe {
        if swizzle {
            let s = src as *const u8;
            let d = *dst;
            for i in (0..size).step_by(4) {
                *d.add(i) = *s.add(i + 2); // B <- R
                *d.add(i + 1) = *s.add(i + 1); // G
                *d.add(i + 2) = *s.add(i); // R <- B
                *d.add(i + 3) = *s.add(i + 3);
            }
        } else {
            std::ptr::copy_nonoverlapping(src as *const u8, *dst, size);
        }
        libc::munmap(src, size);
    }

    if std::env::var_os("ARO_DUMP_FRAMES").is_some() {
        // Dump the presented pixels (already XRGB/XBGR in the shm) as a PPM.
        let (_, _, dst, _) = app.pool.as_ref().unwrap();
        let path = format!("/tmp/claude-1000/-home-scttymn-Work/acedbb5c-a659-4a7b-ad88-ecf6b70d7242/scratchpad/frame-{}.ppm", buf.id);
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        unsafe {
            for i in (0..size).step_by(4) {
                // shm holds XRGB or XBGR; read back as the wl format we chose.
                let (b0, b1, b2) = (*(*dst).add(i), *(*dst).add(i + 1), *(*dst).add(i + 2));
                // Convert to RGB for the PPM regardless of swizzle: our shm is B,G,R,X (xrgb) or R,G,B,X (xbgr).
                if swizzle { out.extend_from_slice(&[b2, b1, b0]); } else { out.extend_from_slice(&[b0, b1, b2]); }
            }
        }
        let _ = std::fs::write(&path, out);
    }
    let surface = app.surface.as_ref().unwrap();
    surface.attach(app.wl_buffer.as_ref(), 0, 0);
    surface.damage_buffer(0, 0, w as i32, h as i32);
    surface.commit();
    app.buffer_busy = true;
    log::info!("compositor: presented buffer {} ({w}x{h})", buf.id);
}

// --- Global binding ---
impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(app: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
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

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(app: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            app.buffer_busy = false;
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
            }
            Event::Motion { surface_x, surface_y, .. } => {
                app.ptr_x = surface_x as f32 * app.host.scale as f32;
                app.ptr_y = surface_y as f32 * app.host.scale as f32;
                if app.ptr_down {
                    app.input.send_motion(ACTION_MOVE, app.ptr_x, app.ptr_y);
                }
            }
            Event::Button { button, state, .. } => {
                if button != BTN_LEFT {
                    return;
                }
                match state {
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        app.ptr_down = true;
                        app.input.send_motion(ACTION_DOWN, app.ptr_x, app.ptr_y);
                    }
                    wayland_client::WEnum::Value(wl_pointer::ButtonState::Released) => {
                        app.ptr_down = false;
                        app.input.send_motion(ACTION_UP, app.ptr_x, app.ptr_y);
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
