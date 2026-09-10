//! `media.player`: android.media.IMediaPlayerService and android.media.IMediaPlayer.
//! Host media player service that connects to PipeWire for audio playback.
use super::Service;
use crate::services;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub struct MediaPlayerService;

impl Service for MediaPlayerService {
    const DESCRIPTOR: &'static str = "android.media.IMediaPlayerService";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "CREATE"),
        (2, "CREATE_MEDIA_RECORDER"),
        (3, "CREATE_METADATA_RETRIEVER"),
        (4, "ADD_BATTERY_DATA"),
        (5, "PULL_BATTERY_DATA"),
        (6, "LISTEN_FOR_REMOTE_DISPLAY"),
        (7, "GET_CODEC_LIST"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "CREATE" => {
                let client: Option<SIBinder> = data.read().ok().flatten();
                let audio_session_id = data.read_i32().unwrap_or(0);
                log::info!("media.player: CREATE client={:?} session={audio_session_id}", client.is_some());
                let player = MediaPlayerInstance::new(client, audio_session_id);
                let binder = services::binder_of(player);
                reply.write(&Some(binder))?;
                Ok(true)
            }
            "CREATE_METADATA_RETRIEVER" => {
                log::info!("media.player: CREATE_METADATA_RETRIEVER");
                let retriever = MetadataRetrieverInstance;
                let binder = services::binder_of(retriever);
                reply.write(&Some(binder))?;
                Ok(true)
            }
            _ => {
                log::warn!("media.player: unhandled {name}");
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}

pub struct MetadataRetrieverInstance;

impl Service for MetadataRetrieverInstance {
    const DESCRIPTOR: &'static str = "android.media.IMediaMetadataRetriever";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "DISCONNECT"),
        (2, "SET_DATA_SOURCE_URL"),
        (3, "SET_DATA_SOURCE_FD"),
        (4, "SET_DATA_SOURCE_CALLBACK"),
        (5, "GET_FRAME_AT_TIME"),
        (6, "GET_IMAGE_AT_INDEX"),
        (7, "GET_IMAGE_RECT_AT_INDEX"),
        (8, "GET_FRAME_AT_INDEX"),
        (9, "EXTRACT_ALBUM_ART"),
        (10, "EXTRACT_METADATA"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "DISCONNECT" => {
                reply.write_i32(0)?;
                Ok(true)
            }
            "SET_DATA_SOURCE_FD" | "SET_DATA_SOURCE_URL" | "SET_DATA_SOURCE_CALLBACK" => {
                reply.write_i32(0)?; // OK
                Ok(true)
            }
            "GET_FRAME_AT_TIME" | "GET_IMAGE_AT_INDEX" | "GET_IMAGE_RECT_AT_INDEX" | "GET_FRAME_AT_INDEX" | "EXTRACT_ALBUM_ART" => {
                reply.write(&Option::<SIBinder>::None)?;
                Ok(true)
            }
            "EXTRACT_METADATA" => {
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => {
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}

fn notify_client(client: &Option<SIBinder>, msg: i32, ext1: i32, ext2: i32) {
    if let Some(client) = client {
        if let Some(proxy) = client.as_proxy() {
            if let Ok(mut d) = proxy.prepare_transact(false) {
                let _ = d.write_interface_token("android.media.IMediaPlayerClient");
                let _ = d.write_i32(msg);
                let _ = d.write_i32(ext1);
                let _ = d.write_i32(ext2);
                let _ = proxy.submit_transact(1, &d, rsbinder::FLAG_ONEWAY);
            }
        }
    }
}

static NEXT_PLAYER_ID: AtomicU64 = AtomicU64::new(1);

struct Inner {
    id: u64,
    client: Option<SIBinder>,
    #[allow(dead_code)]
    audio_session_id: i32,
    temp_path: Option<std::path::PathBuf>,
    looping: bool,
    volume: f32,
    stop_signal: Arc<AtomicBool>,
    current_child: Arc<Mutex<Option<std::process::Child>>>,
    start_time: Option<std::time::Instant>,
    paused_position_ms: i32,
    duration_ms: i32,
    ipc_socket: Option<std::path::PathBuf>,
}

impl Inner {
    fn kill_child(&mut self) {
        self.stop_signal.store(true, Ordering::SeqCst);
        if let Some(sock) = self.ipc_socket.take() {
            let _ = std::fs::remove_file(sock);
        }
        if let Ok(mut g) = self.current_child.lock() {
            if let Some(mut child) = g.take() {
                let pid = child.id() as i32;
                log::info!("media.player[{}]: killing playback process pid={pid}", self.id);
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                    libc::kill(pid, libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.kill_child();
        if let Some(p) = self.temp_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub struct MediaPlayerInstance {
    inner: Arc<Mutex<Inner>>,
    playing: Arc<AtomicBool>,
}

impl MediaPlayerInstance {
    pub fn new(client: Option<SIBinder>, audio_session_id: i32) -> Self {
        let id = NEXT_PLAYER_ID.fetch_add(1, Ordering::SeqCst);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                id,
                client,
                audio_session_id,
                temp_path: None,
                looping: false,
                volume: 1.0,
                stop_signal: Arc::new(AtomicBool::new(false)),
                current_child: Arc::new(Mutex::new(None)),
                start_time: None,
                paused_position_ms: 0,
                duration_ms: 30000,
                ipc_socket: None,
            })),
            playing: Arc::new(AtomicBool::new(false)),
        }
    }

    fn spawn_playback_thread(&self, start_offset_ms: i32) {
        let sock_path = std::env::temp_dir().join(format!("aro_mpv_{}_{}.sock", NEXT_PLAYER_ID.load(Ordering::SeqCst), std::process::id()));
        let _ = std::fs::remove_file(&sock_path);

        let (id, new_stop, file_to_play, looping, child_slot, volume, client) = {
            let mut inner = self.inner.lock().unwrap();
            inner.kill_child();
            let new_stop = Arc::new(AtomicBool::new(false));
            inner.stop_signal = new_stop.clone();
            inner.start_time = Some(std::time::Instant::now());
            inner.paused_position_ms = start_offset_ms;
            inner.ipc_socket = Some(sock_path.clone());
            (
                inner.id,
                new_stop,
                inner.temp_path.clone(),
                inner.looping,
                inner.current_child.clone(),
                inner.volume,
                inner.client.clone(),
            )
        };
        log::info!("media.player[{id}]: START playback offset={start_offset_ms}ms (looping={looping}, volume={volume:.2})");
        self.playing.store(true, Ordering::SeqCst);
        let playing_flag = self.playing.clone();
        let inner_arc = self.inner.clone();

        std::thread::spawn(move || {
            let fallback_path = std::path::PathBuf::from("/home/scttymn/.local/share/aro/system/system/product/media/audio/alarms/Alarm_Beep_01.ogg");
            let path = file_to_play.as_ref().unwrap_or(&fallback_path);
            let mut cur_offset = start_offset_ms as f64 / 1000.0;

            while !new_stop.load(Ordering::SeqCst) {
                log::info!("media.player[{id}]: spawning mpv on {} (offset={cur_offset:.2}s, volume={volume:.2})", path.display());
                let mut cmd = std::process::Command::new("mpv");
                cmd.arg("--no-video")
                    .arg("--no-terminal")
                    .arg(format!("--input-ipc-server={}", sock_path.display()))
                    .arg(format!("--start={cur_offset:.3}"))
                    .arg(format!("--volume={}", (volume * 100.0) as u32))
                    .arg(path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());

                unsafe {
                    cmd.pre_exec(|| {
                        libc::setpgid(0, 0);
                        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                        Ok(())
                    });
                }

                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        log::warn!("media.player[{id}]: failed to spawn mpv: {e}");
                        break;
                    }
                };

                let pid = child.id() as i32;
                {
                    let mut g = child_slot.lock().unwrap();
                    if new_stop.load(Ordering::SeqCst) {
                        log::info!("media.player[{id}]: stop requested right after spawn, killing pid={pid}");
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                            libc::kill(pid, libc::SIGKILL);
                        }
                        let mut c = child;
                        let _ = c.kill();
                        let _ = c.wait();
                        break;
                    }
                    *g = Some(child);
                }

                loop {
                    if new_stop.load(Ordering::SeqCst) {
                        if let Ok(mut g) = child_slot.lock() {
                            if let Some(mut c) = g.take() {
                                let pid = c.id() as i32;
                                unsafe {
                                    libc::kill(-pid, libc::SIGKILL);
                                    libc::kill(pid, libc::SIGKILL);
                                }
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                        }
                        break;
                    }
                    let finished = if let Ok(mut g) = child_slot.lock() {
                        if let Some(ref mut c) = *g {
                            match c.try_wait() {
                                Ok(Some(_)) => true,
                                Ok(None) => false,
                                Err(_) => true,
                            }
                        } else {
                            true
                        }
                    } else {
                        true
                    };
                    if finished {
                        if let Ok(mut g) = child_slot.lock() {
                            g.take();
                        }
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }

                if !looping || new_stop.load(Ordering::SeqCst) {
                    break;
                }
                cur_offset = 0.0;
            }

            let was_stopped = new_stop.load(Ordering::SeqCst);
            playing_flag.store(false, Ordering::SeqCst);
            log::info!("media.player[{id}]: playback finished (stopped={was_stopped})");

            let _ = std::fs::remove_file(&sock_path);
            if !was_stopped {
                if let Ok(mut inner) = inner_arc.lock() {
                    inner.start_time = None;
                    inner.paused_position_ms = inner.duration_ms;
                    inner.ipc_socket = None;
                }
                notify_client(&client, 2, 0, 0); // MEDIA_PLAYBACK_COMPLETE = 2
            }
        });
    }
}

impl Service for MediaPlayerInstance {
    const DESCRIPTOR: &'static str = "android.media.IMediaPlayer";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "DISCONNECT"),
        (2, "SET_DATA_SOURCE_URL"),
        (3, "SET_DATA_SOURCE_FD"),
        (4, "SET_DATA_SOURCE_STREAM"),
        (5, "SET_DATA_SOURCE_CALLBACK"),
        (6, "SET_DATA_SOURCE_RTP"),
        (7, "SET_BUFFERING_SETTINGS"),
        (8, "GET_BUFFERING_SETTINGS"),
        (9, "PREPARE_ASYNC"),
        (10, "START"),
        (11, "STOP"),
        (12, "IS_PLAYING"),
        (13, "SET_PLAYBACK_SETTINGS"),
        (14, "GET_PLAYBACK_SETTINGS"),
        (15, "SET_SYNC_SETTINGS"),
        (16, "GET_SYNC_SETTINGS"),
        (17, "PAUSE"),
        (18, "SEEK_TO"),
        (19, "GET_CURRENT_POSITION"),
        (20, "GET_DURATION"),
        (21, "RESET"),
        (22, "NOTIFY_AT"),
        (23, "SET_AUDIO_STREAM_TYPE"),
        (24, "SET_LOOPING"),
        (25, "SET_VOLUME"),
        (26, "INVOKE"),
        (27, "SET_METADATA_FILTER"),
        (28, "GET_METADATA"),
        (29, "SET_AUX_EFFECT_SEND_LEVEL"),
        (30, "ATTACH_AUX_EFFECT"),
        (31, "SET_VIDEO_SURFACETEXTURE"),
        (32, "SET_PARAMETER"),
        (33, "GET_PARAMETER"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "SET_DATA_SOURCE_FD" => {
                match data.read_raw_file_descriptor() {
                    Ok(owned_fd) => {
                        let offset = data.read_i64().unwrap_or(0);
                        let length = data.read_i64().unwrap_or(-1);
                        let raw = owned_fd.as_raw_fd();
                        log::info!("media.player: SET_DATA_SOURCE_FD fd={raw} offset={offset} length={length}");
                        let mut f = std::fs::File::from(owned_fd);
                        if offset > 0 {
                            let _ = f.seek(SeekFrom::Start(offset as u64));
                        }
                        let mut buf = Vec::new();
                        if length > 0 && length < 50_000_000 {
                            buf.resize(length as usize, 0);
                            if let Ok(_) = f.read_exact(&mut buf) {
                                log::info!("media.player: read {} bytes from fd", buf.len());
                            }
                        } else {
                            if let Ok(_) = f.read_to_end(&mut buf) {
                                log::info!("media.player: read {} bytes (to EOF) from fd", buf.len());
                            }
                        }
                        if !buf.is_empty() {
                            let mut inner = self.inner.lock().unwrap();
                            if let Some(old) = inner.temp_path.take() {
                                let _ = std::fs::remove_file(old);
                            }
                            let temp = std::env::temp_dir().join(format!("aro_audio_{}_{}.ogg", inner.id, std::process::id()));
                            if let Ok(_) = std::fs::write(&temp, &buf) {
                                log::info!("media.player[{}]: saved audio data to {}", inner.id, temp.display());
                                let duration_ms = std::process::Command::new("ffprobe")
                                    .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
                                    .arg(&temp)
                                    .output()
                                    .ok()
                                    .and_then(|o| String::from_utf8(o.stdout).ok())
                                    .and_then(|s| s.trim().parse::<f64>().ok())
                                    .map(|secs| (secs * 1000.0) as i32)
                                    .unwrap_or(30000);
                                inner.duration_ms = duration_ms;
                                inner.paused_position_ms = 0;
                                inner.start_time = None;
                                inner.temp_path = Some(temp);
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("media.player: failed to read raw fd: {e:?}");
                    }
                }
                reply.write_i32(0)?;
                Ok(true)
            }
            "SET_DATA_SOURCE_URL" => {
                log::info!("media.player: SET_DATA_SOURCE_URL");
                reply.write_i32(0)?;
                Ok(true)
            }
            "PREPARE_ASYNC" => {
                log::info!("media.player: PREPARE_ASYNC -> notifying client MEDIA_PREPARED");
                let client = self.inner.lock().unwrap().client.clone();
                notify_client(&client, 1, 0, 0); // MEDIA_PREPARED = 1
                reply.write_i32(0)?;
                Ok(true)
            }
            "START" => {
                let (start_pos, dur, ipc_sock) = {
                    let inner = self.inner.lock().unwrap();
                    (inner.paused_position_ms, inner.duration_ms, inner.ipc_socket.clone())
                };
                let pos = if start_pos >= dur - 100 { 0 } else { start_pos };
                let mut resumed = false;
                if let Some(ref sock) = ipc_sock {
                    if let Ok(mut s) = UnixStream::connect(sock) {
                        let _ = writeln!(s, "{{\"command\": [\"seek\", {:.3}, \"absolute\"]}}", pos as f64 / 1000.0);
                        let _ = writeln!(s, "{{\"command\": [\"set_property\", \"pause\", false]}}");
                        let mut inner = self.inner.lock().unwrap();
                        inner.start_time = Some(std::time::Instant::now());
                        inner.paused_position_ms = pos;
                        self.playing.store(true, Ordering::SeqCst);
                        resumed = true;
                    }
                }
                if !resumed {
                    self.spawn_playback_thread(pos);
                }
                reply.write_i32(0)?;
                Ok(true)
            }
            "STOP" => {
                let mut inner = self.inner.lock().unwrap();
                let id = inner.id;
                log::info!("media.player[{id}]: STOP");
                inner.kill_child();
                inner.start_time = None;
                inner.paused_position_ms = 0;
                self.playing.store(false, Ordering::SeqCst);
                reply.write_i32(0)?;
                Ok(true)
            }
            "PAUSE" => {
                let mut inner = self.inner.lock().unwrap();
                let id = inner.id;
                log::info!("media.player[{id}]: PAUSE");
                if let Some(t) = inner.start_time.take() {
                    inner.paused_position_ms += t.elapsed().as_millis() as i32;
                }
                self.playing.store(false, Ordering::SeqCst);
                if let Some(ref sock) = inner.ipc_socket {
                    if let Ok(mut s) = UnixStream::connect(sock) {
                        let _ = writeln!(s, "{{\"command\": [\"set_property\", \"pause\", true]}}");
                    }
                }
                reply.write_i32(0)?;
                Ok(true)
            }
            "SEEK_TO" => {
                let msec = data.read_i32().unwrap_or(0);
                let _mode = data.read_i32().unwrap_or(0);
                log::info!("media.player: SEEK_TO msec={msec}");
                let is_playing = self.playing.load(Ordering::SeqCst);
                let ipc_sock = self.inner.lock().unwrap().ipc_socket.clone();
                let mut seeked_live = false;
                if let Some(ref sock) = ipc_sock {
                    if let Ok(mut s) = UnixStream::connect(sock) {
                        let _ = writeln!(s, "{{\"command\": [\"seek\", {:.3}, \"absolute\"]}}", msec as f64 / 1000.0);
                        if !is_playing {
                            let _ = writeln!(s, "{{\"command\": [\"set_property\", \"pause\", true]}}");
                        }
                        seeked_live = true;
                    }
                }
                {
                    let mut inner = self.inner.lock().unwrap();
                    inner.paused_position_ms = msec;
                    if is_playing {
                        inner.start_time = Some(std::time::Instant::now());
                    } else {
                        inner.start_time = None;
                    }
                }
                if is_playing && !seeked_live {
                    self.spawn_playback_thread(msec);
                }
                let client = self.inner.lock().unwrap().client.clone();
                notify_client(&client, 4, 0, 0); // MEDIA_SEEK_COMPLETE = 4
                reply.write_i32(0)?;
                Ok(true)
            }
            "IS_PLAYING" => {
                let is_playing = self.playing.load(Ordering::SeqCst);
                reply.write_i32(if is_playing { 1 } else { 0 })?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "SET_LOOPING" => {
                let loop_val = data.read_i32().unwrap_or(0);
                let mut inner = self.inner.lock().unwrap();
                let id = inner.id;
                log::info!("media.player[{id}]: SET_LOOPING {loop_val}");
                inner.looping = loop_val != 0;
                reply.write_i32(0)?;
                Ok(true)
            }
            "SET_VOLUME" => {
                let left = data.read_f32().unwrap_or(1.0);
                let right = data.read_f32().unwrap_or(1.0);
                let vol = ((left + right) / 2.0).clamp(0.0, 1.0);
                let mut inner = self.inner.lock().unwrap();
                let id = inner.id;
                log::info!("media.player[{id}]: SET_VOLUME left={left} right={right} -> {vol}");
                inner.volume = vol;
                reply.write_i32(0)?;
                Ok(true)
            }
            "GET_DURATION" => {
                let dur = self.inner.lock().unwrap().duration_ms;
                reply.write_i32(dur)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "GET_CURRENT_POSITION" => {
                let pos = {
                    let inner = self.inner.lock().unwrap();
                    let elapsed = if self.playing.load(Ordering::SeqCst) {
                        inner.start_time.map(|t| t.elapsed().as_millis() as i32).unwrap_or(0)
                    } else {
                        0
                    };
                    (inner.paused_position_ms + elapsed).min(inner.duration_ms)
                };
                reply.write_i32(pos)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "RESET" | "DISCONNECT" => {
                let mut inner = self.inner.lock().unwrap();
                let id = inner.id;
                log::info!("media.player[{id}]: RESET/DISCONNECT");
                inner.kill_child();
                inner.start_time = None;
                inner.paused_position_ms = 0;
                if let Some(old) = inner.temp_path.take() {
                    let _ = std::fs::remove_file(old);
                }
                self.playing.store(false, Ordering::SeqCst);
                reply.write_i32(0)?;
                Ok(true)
            }
            "GET_PLAYBACK_SETTINGS" => {
                reply.write_i32(0)?; // OK
                reply.write_f32(1.0)?; // speed
                reply.write_f32(1.0)?; // pitch
                reply.write_i32(0)?; // fallback
                reply.write_i32(0)?; // stretch
                Ok(true)
            }
            "GET_BUFFERING_SETTINGS" => {
                reply.write_i32(0)?; // OK
                reply.write_i32(0)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => {
                log::debug!("media.player: unhandled {name}");
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}
