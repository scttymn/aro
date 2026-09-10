//! `media`: MediaProvider (`android.content.IContentProvider`) and MediaStore.
//!
//! Indexes system alarm/ringtone/notification sounds from the system image
//! (and user ~/Alarms if present) plus host pictures and videos from XDG dirs
//! (`~/Pictures`, `~/Videos`, `~/Downloads`, `~/DCIM`). Serves them via standard
//! Android CursorWindow and ParcelFileDescriptor IPC so RingtoneManager,
//! DeskClock, Music, and Gallery2 can enumerate and open files.
//!
//! Visual items are exposed at Android paths under `/storage/emulated/0/…`,
//! which `aro-exec` bind-mounts from `$ARO_SDCARD` (the host home). Gallery2
//! reads `_data` with `BitmapFactory`, so those paths have to be real files
//! inside the app namespace.

use super::cursor::{write_query_reply, CellValue, CursorWindowBuilder};
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MEDIA_TYPE_IMAGE: i32 = 1;
const MEDIA_TYPE_AUDIO: i32 = 2;
const MEDIA_TYPE_VIDEO: i32 = 3;

const MAX_VISUAL_ITEMS: usize = 2000;
const MAX_WALK_DEPTH: u32 = 6;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AudioItem {
    pub id: i64,
    pub title: String,
    pub display_name: String,
    pub path: PathBuf,
    pub container_path: String,
    pub is_alarm: bool,
    pub is_ringtone: bool,
    pub is_notification: bool,
}

#[derive(Debug, Clone)]
pub struct VisualItem {
    pub id: i64,
    pub title: String,
    pub display_name: String,
    pub path: PathBuf,
    pub container_path: String,
    pub relative_path: String,
    pub mime_type: String,
    pub media_type: i32,
    pub bucket_id: i32,
    pub bucket_display_name: String,
    pub size: i64,
    pub date_added: i64,
    pub date_modified: i64,
    pub date_taken: i64,
    pub width: i32,
    pub height: i32,
    pub orientation: i32,
    pub duration_ms: i64,
}

pub struct MediaService {
    pub items: std::sync::RwLock<Vec<AudioItem>>,
    pub visual: std::sync::RwLock<Vec<VisualItem>>,
    pub next_id: std::sync::atomic::AtomicI64,
    custom_path: PathBuf,
}

impl MediaService {
    pub fn load(system_dir: &Path, data_dir: &Path) -> Self {
        let mut items = Vec::new();
        let mut next_id = 1i64;

        let base = system_dir.join("system/product/media/audio");
        let categories = [
            ("alarms", true, false, false),
            ("ringtones", false, true, false),
            ("notifications", false, false, true),
        ];

        for (dir_name, is_alarm, is_ringtone, is_notification) in categories {
            let cat_dir = base.join(dir_name);
            if let Ok(entries) = std::fs::read_dir(&cat_dir) {
                let mut files: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().map_or(false, |ext| ext == "ogg"))
                    .collect();
                files.sort();

                for path in files {
                    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let title = file_stem.replace('_', " ");
                    let display_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    let container_path = format!("/product/media/audio/{dir_name}/{display_name}");

                    items.push(AudioItem {
                        id: next_id,
                        title,
                        display_name,
                        path,
                        container_path,
                        is_alarm,
                        is_ringtone,
                        is_notification,
                    });
                    next_id += 1;
                }
            }
        }

        // Also check if user has a ~/Alarms folder on the host
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let user_alarms = home.join("Alarms");
            if let Ok(entries) = std::fs::read_dir(&user_alarms) {
                let mut files: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.extension().map_or(false, |ext| {
                            let ext_str = ext.to_string_lossy().to_ascii_lowercase();
                            ext_str == "ogg" || ext_str == "mp3" || ext_str == "wav"
                        })
                    })
                    .collect();
                files.sort();

                for path in files {
                    let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    let title = file_stem.replace('_', " ");
                    let display_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    let container_path = format!("/sdcard/Alarms/{display_name}");

                    items.push(AudioItem {
                        id: next_id,
                        title,
                        display_name,
                        path,
                        container_path,
                        is_alarm: true,
                        is_ringtone: false,
                        is_notification: false,
                    });
                    next_id += 1;
                }
            }
        }

        let custom_path = data_dir.join("system/custom_media.json");
        if custom_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&custom_path) {
                if let Ok(saved_items) = serde_json::from_str::<Vec<AudioItem>>(&content) {
                    for item in saved_items {
                        if item.id >= next_id {
                            next_id = item.id + 1;
                        }
                        items.push(item);
                    }
                }
            }
        }

        let visual = index_visual_media(&mut next_id);
        let n_img = visual.iter().filter(|v| v.media_type == MEDIA_TYPE_IMAGE).count();
        let n_vid = visual.iter().filter(|v| v.media_type == MEDIA_TYPE_VIDEO).count();
        log::info!(
            "media: loaded {} audio items ({} alarms), {} images, {} videos",
            items.len(),
            items.iter().filter(|i| i.is_alarm).count(),
            n_img,
            n_vid
        );
        if n_img == 0 {
            log::warn!("media: no host images indexed; put photos in ~/Pictures (or $XDG_PICTURES_DIR) for Gallery");
        }

        Self {
            items: std::sync::RwLock::new(items),
            visual: std::sync::RwLock::new(visual),
            next_id: std::sync::atomic::AtomicI64::new(next_id),
            custom_path,
        }
    }

    fn save_custom_items(&self) {
        let items = self.items.read().unwrap();
        let custom: Vec<AudioItem> = items.iter().filter(|i| !i.container_path.starts_with("/product/")).cloned().collect();
        if let Some(parent) = self.custom_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&custom) {
            let _ = std::fs::write(&self.custom_path, json);
        }
    }

    pub fn add_custom_file(&self, path: PathBuf) -> AudioItem {
        {
            let items = self.items.read().unwrap();
            if let Some(existing) = items.iter().find(|i| i.path == path) {
                return existing.clone();
            }
        }
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Custom Sound");
        let title = file_stem.replace('_', " ");
        let display_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("sound.ogg").to_string();
        let item = AudioItem {
            id,
            title,
            display_name: display_name.clone(),
            container_path: format!("/sdcard/{display_name}"),
            path,
            is_alarm: true,
            is_ringtone: true,
            is_notification: true,
        };
        self.items.write().unwrap().push(item.clone());
        self.save_custom_items();
        log::info!("media: added custom audio item id={id} title={:?} path={:?}", item.title, item.path);
        item
    }
}

/// Java `String.hashCode` (signed 32-bit). MediaProvider.computeBucketValues
/// hashes the lowercased parent path this way.
fn java_hash_code(s: &str) -> i32 {
    let mut h: i32 = 0;
    for u in s.encode_utf16() {
        h = h.wrapping_mul(31).wrapping_add(u as i32);
    }
    h
}

fn bucket_of(container_path: &str) -> (i32, String) {
    let parent = container_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/");
    let name = parent.rsplit('/').next().unwrap_or("").to_string();
    (java_hash_code(&parent.to_lowercase()), name)
}

fn mime_for_ext(ext: &str) -> Option<(&'static str, i32)> {
    Some(match ext {
        "jpg" | "jpeg" | "jpe" => ("image/jpeg", MEDIA_TYPE_IMAGE),
        "png" => ("image/png", MEDIA_TYPE_IMAGE),
        "gif" => ("image/gif", MEDIA_TYPE_IMAGE),
        "webp" => ("image/webp", MEDIA_TYPE_IMAGE),
        "bmp" => ("image/bmp", MEDIA_TYPE_IMAGE),
        "heic" => ("image/heic", MEDIA_TYPE_IMAGE),
        "heif" => ("image/heif", MEDIA_TYPE_IMAGE),
        "avif" => ("image/avif", MEDIA_TYPE_IMAGE),
        "mp4" | "m4v" => ("video/mp4", MEDIA_TYPE_VIDEO),
        "webm" => ("video/webm", MEDIA_TYPE_VIDEO),
        "mkv" => ("video/x-matroska", MEDIA_TYPE_VIDEO),
        "3gp" => ("video/3gpp", MEDIA_TYPE_VIDEO),
        "mov" => ("video/quicktime", MEDIA_TYPE_VIDEO),
        "avi" => ("video/avi", MEDIA_TYPE_VIDEO),
        _ => return None,
    })
}

fn xdg_user_dir(home: &Path, key: &str, fallback: &str) -> PathBuf {
    let cfg = home.join(".config/user-dirs.dirs");
    if let Ok(text) = std::fs::read_to_string(&cfg) {
        let prefix = format!("{key}=");
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix(&prefix) {
                let rest = rest.trim().trim_matches('"');
                let path = rest.replace("$HOME", &home.to_string_lossy());
                return PathBuf::from(path);
            }
        }
    }
    home.join(fallback)
}

fn host_sdcard() -> Option<PathBuf> {
    std::env::var_os("ARO_SDCARD")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn container_path_for(sdcard_host: &Path, host_path: &Path) -> Option<String> {
    let rel = host_path.strip_prefix(sdcard_host).ok()?;
    let rel = rel.to_string_lossy().replace('\\', "/");
    Some(format!("/storage/emulated/0/{rel}"))
}

fn unix_times(meta: &std::fs::Metadata) -> (i64, i64, i64) {
    use std::time::UNIX_EPOCH;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let added = meta
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(modified);
    (added, modified, modified * 1000)
}

/// Read PNG/JPEG/GIF dimensions from the file header. Unknown formats stay 0.
fn image_dimensions(path: &Path) -> (i32, i32) {
    let Ok(mut f) = std::fs::File::open(path) else {
        return (0, 0);
    };
    use std::io::Read;
    let mut buf = [0u8; 32];
    let n = f.read(&mut buf).unwrap_or(0);
    let b = &buf[..n];
    if n >= 24 && b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) && &b[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(b[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(b[20..24].try_into().unwrap());
        return (w as i32, h as i32);
    }
    if n >= 10 && b.starts_with(b"GIF8") {
        let w = u16::from_le_bytes([b[6], b[7]]) as i32;
        let h = u16::from_le_bytes([b[8], b[9]]) as i32;
        return (w, h);
    }
    if n >= 2 && b[0] == 0xFF && b[1] == 0xD8 {
        return jpeg_dimensions(path);
    }
    (0, 0)
}

fn jpeg_dimensions(path: &Path) -> (i32, i32) {
    let Ok(data) = std::fs::read(path) else {
        return (0, 0);
    };
    let data = if data.len() > 256 * 1024 { &data[..256 * 1024] } else { &data };
    let mut i = 2usize;
    while i + 8 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // SOI / EOI / RSTn have no length
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if i + 3 >= data.len() {
            break;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        // SOF0 / SOF1 / SOF2
        if (0xC0..=0xC3).contains(&marker) && i + 8 < data.len() {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as i32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as i32;
            return (w, h);
        }
        i = i.saturating_add(2 + len);
    }
    (0, 0)
}

fn visual_from_path(path: PathBuf, sdcard_host: &Path, id: i64) -> Option<VisualItem> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let (mime_type, media_type) = mime_for_ext(&ext)?;
    let container_path = container_path_for(sdcard_host, &path)?;
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let size = meta.len() as i64;
    let (date_added, date_modified, date_taken) = unix_times(&meta);
    let (width, height) = if media_type == MEDIA_TYPE_IMAGE {
        image_dimensions(&path)
    } else {
        (0, 0)
    };
    let display_name = path.file_name()?.to_string_lossy().into_owned();
    let title = path.file_stem()?.to_string_lossy().into_owned();
    let (bucket_id, bucket_display_name) = bucket_of(&container_path);
    let parent = container_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/storage/emulated/0");
    let relative = parent
        .strip_prefix("/storage/emulated/0/")
        .unwrap_or("")
        .to_string();
    let relative_path = if relative.is_empty() {
        String::new()
    } else {
        format!("{relative}/")
    };
    Some(VisualItem {
        id,
        title,
        display_name,
        path,
        container_path,
        relative_path,
        mime_type: mime_type.to_string(),
        media_type,
        bucket_id,
        bucket_display_name,
        size,
        date_added,
        date_modified,
        date_taken,
        width,
        height,
        orientation: 0,
        duration_ms: 0,
    })
}

fn walk_visual(dir: &Path, sdcard_host: &Path, items: &mut Vec<VisualItem>, next_id: &mut i64, depth: u32) {
    if depth > MAX_WALK_DEPTH || items.len() >= MAX_VISUAL_ITEMS {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        let path = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => dirs.push(path),
            Ok(t) if t.is_file() => files.push(path),
            _ => {
                if path.is_dir() {
                    dirs.push(path);
                } else if path.is_file() {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    dirs.sort();
    for path in files {
        if items.len() >= MAX_VISUAL_ITEMS {
            return;
        }
        if let Some(item) = visual_from_path(path, sdcard_host, *next_id) {
            *next_id += 1;
            items.push(item);
        }
    }
    for d in dirs {
        walk_visual(&d, sdcard_host, items, next_id, depth + 1);
    }
}

fn index_visual_media(next_id: &mut i64) -> Vec<VisualItem> {
    let Some(sdcard_host) = host_sdcard() else {
        return Vec::new();
    };
    let mut roots = vec![
        xdg_user_dir(&sdcard_host, "XDG_PICTURES_DIR", "Pictures"),
        xdg_user_dir(&sdcard_host, "XDG_VIDEOS_DIR", "Videos"),
        xdg_user_dir(&sdcard_host, "XDG_DOWNLOAD_DIR", "Downloads"),
        sdcard_host.join("DCIM"),
        sdcard_host.join("Download"),
        sdcard_host.join("Movies"),
    ];
    // Unique existing directories, still under the sdcard root.
    roots.sort();
    roots.dedup();
    let mut items = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        if root.strip_prefix(&sdcard_host).is_err() && root != sdcard_host {
            continue;
        }
        walk_visual(&root, &sdcard_host, &mut items, next_id, 0);
    }
    items.sort_by(|a, b| b.date_taken.cmp(&a.date_taken).then(b.id.cmp(&a.id)));
    items
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UriKind {
    Audio { id: Option<i64>, external: bool },
    Images { id: Option<i64> },
    Videos { id: Option<i64> },
    Files { id: Option<i64> },
    Thumbnails,
    Unknown { id: Option<i64>, external: bool },
}

fn trailing_id(path: &str) -> Option<i64> {
    path.rsplit('/').next().and_then(|s| s.parse().ok())
}

fn classify_uri(uri: &str) -> UriKind {
    let path = uri.split('?').next().unwrap_or(uri);
    let lower = path.to_ascii_lowercase();
    let external = lower.contains("external");
    if lower.contains("/images/thumbnails") || lower.contains("/video/thumbnails") {
        UriKind::Thumbnails
    } else if lower.contains("/images/") {
        UriKind::Images { id: trailing_id(path) }
    } else if lower.contains("/video/") {
        UriKind::Videos { id: trailing_id(path) }
    } else if lower.contains("/audio/") {
        UriKind::Audio { id: trailing_id(path), external }
    } else if lower.contains("/file") {
        UriKind::Files { id: trailing_id(path) }
    } else {
        UriKind::Unknown { id: trailing_id(path), external }
    }
}

#[derive(Debug, Default)]
struct QueryFilter {
    id: Option<i64>,
    id_min: Option<i64>,
    id_max: Option<i64>,
    bucket_id: Option<i32>,
    media_type: Option<i32>,
    offset: usize,
    limit: Option<usize>,
}

fn query_arg_map(bundle_bytes: &[u8]) -> std::collections::BTreeMap<String, crate::bundle::Value> {
    crate::bundle::parse(bundle_bytes)
}

fn parse_query_filter(uri: &str, bundle_bytes: &[u8], kind_id: Option<i64>) -> QueryFilter {
    let mut f = QueryFilter { id: kind_id, ..QueryFilter::default() };
    let args = query_arg_map(bundle_bytes);

    let selection = args
        .get("android:query-arg-sql-selection")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let selection_args: Vec<String> = args
        .get("android:query-arg-sql-selection-args")
        .and_then(|v| v.as_strs())
        .map(|s| s.to_vec())
        .unwrap_or_default();

    apply_selection(&mut f, &selection, &selection_args);

    if let Some(n) = args.get("android:query-arg-limit").and_then(|v| v.as_i64()) {
        if n >= 0 {
            f.limit = Some(n as usize);
        }
    }
    if let Some(s) = args.get("android:query-arg-sql-limit").and_then(|v| v.as_str()) {
        apply_limit_spec(&mut f, s);
    }
    if let Some(n) = args.get("android:query-arg-offset").and_then(|v| v.as_i64()) {
        if n >= 0 {
            f.offset = n as usize;
        }
    }

    if let Some(q) = uri.split_once('?').map(|(_, q)| q) {
        for part in q.split('&') {
            let (k, v) = match part.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            if k.eq_ignore_ascii_case("limit") {
                apply_limit_spec(&mut f, v);
            } else if k.eq_ignore_ascii_case("bucketId") || k.eq_ignore_ascii_case("bucket_id") {
                if let Ok(id) = v.parse::<i32>() {
                    f.bucket_id = Some(id);
                }
            }
        }
    }
    f
}

fn apply_limit_spec(f: &mut QueryFilter, spec: &str) {
    let spec = spec.trim();
    if let Some((a, b)) = spec.split_once(',') {
        f.offset = a.trim().parse().unwrap_or(f.offset);
        f.limit = b.trim().parse().ok();
    } else if let Ok(n) = spec.parse::<usize>() {
        f.limit = Some(n);
    }
}

fn tokenize_sql(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in sql.chars() {
        if c.is_whitespace() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else if c == '=' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push("=".into());
        } else if c == '(' || c == ')' {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn apply_selection(f: &mut QueryFilter, sql: &str, args: &[String]) {
    if sql.is_empty() {
        return;
    }
    let mut arg_i = 0usize;
    let mut take = || {
        let v = args.get(arg_i).cloned();
        arg_i += 1;
        v
    };
    let tokens = tokenize_sql(sql);
    let mut i = 0usize;
    while i < tokens.len() {
        if tokens[i].eq_ignore_ascii_case("and") {
            i += 1;
            continue;
        }
        if i + 2 < tokens.len() && tokens[i + 1].eq_ignore_ascii_case("between") {
            let col = &tokens[i];
            let lo = &tokens[i + 2];
            let mut hi = "?";
            let mut next = i + 3;
            if i + 4 < tokens.len() && tokens[i + 3].eq_ignore_ascii_case("and") {
                hi = &tokens[i + 4];
                next = i + 5;
            }
            if col.eq_ignore_ascii_case("_id") {
                f.id_min = resolve_num(lo, &mut take).and_then(|s| s.parse().ok());
                f.id_max = resolve_num(hi, &mut take).and_then(|s| s.parse().ok());
            }
            i = next;
            continue;
        }
        if i + 2 < tokens.len() && tokens[i + 1] == "=" {
            let col = &tokens[i];
            let val = resolve_num(&tokens[i + 2], &mut take);
            if col.eq_ignore_ascii_case("bucket_id") {
                f.bucket_id = val.and_then(|s| s.parse().ok());
            } else if col.eq_ignore_ascii_case("_id") {
                f.id = val.and_then(|s| s.parse().ok());
            } else if col.eq_ignore_ascii_case("media_type") {
                f.media_type = val.and_then(|s| s.parse().ok());
            }
            i += 3;
            continue;
        }
        i += 1;
    }
}

fn resolve_num(rhs: &str, take: &mut impl FnMut() -> Option<String>) -> Option<String> {
    let rhs = rhs.trim();
    if rhs == "?" {
        take()
    } else {
        Some(rhs.trim_matches('\'').trim_matches('"').to_string())
    }
}

fn matches_visual(item: &VisualItem, f: &QueryFilter, type_restrict: Option<i32>) -> bool {
    if let Some(t) = type_restrict.or(f.media_type) {
        if item.media_type != t {
            return false;
        }
    }
    if let Some(id) = f.id {
        if item.id != id {
            return false;
        }
    }
    if let Some(lo) = f.id_min {
        if item.id < lo {
            return false;
        }
    }
    if let Some(hi) = f.id_max {
        if item.id > hi {
            return false;
        }
    }
    if let Some(b) = f.bucket_id {
        if item.bucket_id != b {
            return false;
        }
    }
    true
}

fn is_count_col(col: &str) -> bool {
    let l = col.to_ascii_lowercase().replace(' ', "");
    l.starts_with("count(")
}

fn is_count_only(cols: &[String]) -> bool {
    !cols.is_empty() && cols.iter().all(|c| is_count_col(c))
}

fn visual_cell(item: &VisualItem, col: &str) -> CellValue {
    let l = col.to_ascii_lowercase().replace(' ', "");
    if l.starts_with("max(") && l.contains("datetaken") {
        return CellValue::Integer(item.date_taken);
    }
    if l.starts_with("count(") {
        return CellValue::Integer(1);
    }
    match col {
        "_id" => CellValue::Integer(item.id),
        "title" | "title_key" => CellValue::String(item.title.clone()),
        "_display_name" => CellValue::String(item.display_name.clone()),
        "_data" => CellValue::String(item.container_path.clone()),
        "mime_type" => CellValue::String(item.mime_type.clone()),
        "media_type" => CellValue::Integer(item.media_type as i64),
        "bucket_id" => CellValue::Integer(item.bucket_id as i64),
        "bucket_display_name" => CellValue::String(item.bucket_display_name.clone()),
        "datetaken" | "date_taken" => CellValue::Integer(item.date_taken),
        "date_added" => CellValue::Integer(item.date_added),
        "date_modified" => CellValue::Integer(item.date_modified),
        "_size" => CellValue::Integer(item.size),
        "width" => CellValue::Integer(item.width as i64),
        "height" => CellValue::Integer(item.height as i64),
        "orientation" => CellValue::Integer(item.orientation as i64),
        "duration" => CellValue::Integer(item.duration_ms),
        "resolution" => {
            if item.width > 0 && item.height > 0 {
                CellValue::String(format!("{}x{}", item.width, item.height))
            } else {
                CellValue::Null
            }
        }
        "relative_path" => CellValue::String(item.relative_path.clone()),
        "volume_name" => CellValue::String("external_primary".into()),
        "is_pending" | "is_trashed" | "is_favorite" | "is_drm" | "is_notification" | "is_ringtone" | "is_alarm"
        | "mini_thumb_magic" | "deleted" => CellValue::Integer(0),
        "latitude" | "longitude" => CellValue::Null,
        _ => CellValue::Null,
    }
}

fn default_visual_cols(kind: UriKind) -> Vec<String> {
    match kind {
        UriKind::Videos { .. } => vec![
            "_id".into(),
            "title".into(),
            "mime_type".into(),
            "datetaken".into(),
            "date_added".into(),
            "date_modified".into(),
            "_data".into(),
            "duration".into(),
            "bucket_id".into(),
            "_size".into(),
            "resolution".into(),
        ],
        _ => vec![
            "_id".into(),
            "title".into(),
            "mime_type".into(),
            "datetaken".into(),
            "date_added".into(),
            "date_modified".into(),
            "_data".into(),
            "orientation".into(),
            "bucket_id".into(),
            "_size".into(),
            "width".into(),
            "height".into(),
            "bucket_display_name".into(),
            "media_type".into(),
        ],
    }
}

pub struct MediaProvider {
    pub service: Arc<MediaService>,
    pub bulk_cursor: SIBinder,
}

fn read_uri(p: &mut Parcel) -> Option<String> {
    let type_id = p.read_i32().ok()?;
    match type_id {
        0 => None,
        1 | 2 | 3 => ap::read_string8(p).ok().flatten(),
        _ => None,
    }
}

fn skip_attribution(data: &mut Parcel) {
    let start = data.data_position();
    if let Ok(size) = data.read_i32() {
        if size > 4 {
            let _ = data.set_data_position(start + size as usize);
        }
    }
}

fn read_projection(data: &mut Parcel) -> Vec<String> {
    let proj_len = data.read_i32().unwrap_or(0);
    let mut projection = Vec::new();
    for _ in 0..proj_len.max(0) {
        if let Ok(Some(col)) = data.read::<Option<String>>() {
            projection.push(col);
        }
    }
    projection
}

fn read_bundle_bytes(data: &mut Parcel) -> (i32, Vec<u8>) {
    let bundle_len = data.read_i32().unwrap_or(-1);
    let bundle_bytes = if bundle_len > 0 {
        let mut b = Vec::with_capacity(bundle_len as usize);
        let count = (bundle_len as usize + 3) / 4;
        for _ in 0..count {
            if let Ok(val) = data.read_u32() {
                b.extend_from_slice(&val.to_le_bytes());
            }
        }
        b.truncate(bundle_len as usize);
        b
    } else {
        Vec::new()
    };
    (bundle_len, bundle_bytes)
}

fn empty_window(reply: &mut Parcel, bulk_cursor: &SIBinder, cols: &[String]) -> Result<bool> {
    let window = CursorWindowBuilder::new("media_window", cols.to_vec());
    write_query_reply(reply, bulk_cursor, cols, 0, window)
}

fn mime_for_kind(kind: UriKind, visual: Option<&VisualItem>) -> &'static str {
    if let Some(v) = visual {
        return match v.mime_type.as_str() {
            "image/jpeg" => "image/jpeg",
            "image/png" => "image/png",
            "image/gif" => "image/gif",
            "image/webp" => "image/webp",
            "image/bmp" => "image/bmp",
            "image/heic" => "image/heic",
            "image/heif" => "image/heif",
            "image/avif" => "image/avif",
            "video/mp4" => "video/mp4",
            "video/webm" => "video/webm",
            "video/x-matroska" => "video/x-matroska",
            "video/3gpp" => "video/3gpp",
            "video/quicktime" => "video/quicktime",
            "video/avi" => "video/avi",
            other if other.starts_with("image/") => "image/*",
            other if other.starts_with("video/") => "video/*",
            _ => "application/octet-stream",
        };
    }
    match kind {
        UriKind::Images { id: Some(_) } => "vnd.android.cursor.item/image",
        UriKind::Images { id: None } => "vnd.android.cursor.dir/image",
        UriKind::Videos { id: Some(_) } => "vnd.android.cursor.item/video",
        UriKind::Videos { id: None } => "vnd.android.cursor.dir/video",
        UriKind::Files { .. } => "vnd.android.cursor.dir/media",
        UriKind::Thumbnails => "vnd.android.cursor.dir/image",
        _ => "audio/ogg",
    }
}

impl Service for MediaProvider {
    const DESCRIPTOR: &'static str = "android.content.IContentProvider";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "QUERY"),
        (2, "GET_TYPE"),
        (14, "OPEN_FILE"),
        (15, "OPEN_ASSET_FILE"),
        (21, "CALL"),
        (23, "OPEN_TYPED_ASSET_FILE"),
        (25, "CANONICALIZE"),
        (26, "UNCANONICALIZE"),
        (29, "GET_TYPE_ASYNC"),
        (30, "CANONICALIZE_ASYNC"),
        (31, "UNCANONICALIZE_ASYNC"),
        (32, "GET_TYPE_ANONYMOUS_ASYNC"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "QUERY" => {
                skip_attribution(data);
                let uri = read_uri(data);
                let projection = read_projection(data);
                let (bundle_len, bundle_bytes) = read_bundle_bytes(data);

                log::info!("media: QUERY uri={uri:?} proj={projection:?} bundle_len={bundle_len}");

                let uri_str = uri.as_deref().unwrap_or("");
                let kind = classify_uri(uri_str);
                match kind {
                    UriKind::Images { id } | UriKind::Videos { id } | UriKind::Files { id } => {
                        self.query_visual(kind, id, uri_str, projection, &bundle_bytes, reply)
                    }
                    UriKind::Thumbnails => {
                        let cols = if projection.is_empty() {
                            vec!["_id".into(), "image_id".into(), "kind".into(), "_data".into()]
                        } else {
                            projection
                        };
                        empty_window(reply, &self.bulk_cursor, &cols)
                    }
                    UriKind::Audio { id, external } | UriKind::Unknown { id, external } => {
                        self.query_audio(id, external, uri_str, projection, &bundle_bytes, reply)
                    }
                }
            }
            "OPEN_FILE" | "OPEN_ASSET_FILE" | "OPEN_TYPED_ASSET_FILE" => {
                skip_attribution(data);
                let uri = read_uri(data);
                let mode: Option<String> = data.read().ok().flatten();
                log::info!("media: {name} uri={uri:?} mode={mode:?}");

                let uri_str = uri.as_deref().unwrap_or("");
                let kind = classify_uri(uri_str);
                let id = match kind {
                    UriKind::Audio { id, .. }
                    | UriKind::Images { id }
                    | UriKind::Videos { id }
                    | UriKind::Files { id }
                    | UriKind::Unknown { id, .. } => id,
                    UriKind::Thumbnails => None,
                };

                let open = id.and_then(|id| self.lookup_open(kind, id));
                let Some((path, _mime)) = open else {
                    log::warn!("media: {name} item not found for uri={uri:?}");
                    reply.write_i32(-5)?; // FileNotFoundException
                    ap::string16(reply, Some("File not found"))?;
                    return Ok(true);
                };

                let file = match std::fs::File::open(&path) {
                    Ok(f) => f,
                    Err(e) => {
                        log::error!("media: failed to open file {}: {e}", path.display());
                        reply.write_i32(-5)?;
                        ap::string16(reply, Some(&e.to_string()))?;
                        return Ok(true);
                    }
                };

                let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

                ap::no_exception(reply)?;
                reply.write_i32(1)?; // non-null
                reply.write_i32(0)?; // hasComm = 0
                reply.write_raw_file_descriptor(file.as_fd())?;

                if name != "OPEN_FILE" {
                    reply.write_i64(0)?; // startOffset = 0
                    reply.write_i64(file_size as i64)?; // length
                    reply.write_i32(0)?; // hasExtras = 0
                }
                Ok(true)
            }
            "GET_TYPE" => {
                skip_attribution(data);
                let uri = read_uri(data);
                let uri_str = uri.as_deref().unwrap_or("");
                let kind = classify_uri(uri_str);
                let visual = match kind {
                    UriKind::Images { id: Some(id) } | UriKind::Videos { id: Some(id) } | UriKind::Files { id: Some(id) } => {
                        self.service.visual.read().unwrap().iter().find(|v| v.id == id).cloned()
                    }
                    _ => None,
                };
                ap::no_exception(reply)?;
                ap::string16(reply, Some(mime_for_kind(kind, visual.as_ref())))?;
                Ok(true)
            }
            "CANONICALIZE" | "UNCANONICALIZE" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // null Uri: tells ContentResolver to keep current Uri
                Ok(true)
            }
            "GET_TYPE_ASYNC" | "GET_TYPE_ANONYMOUS_ASYNC" => {
                skip_attribution(data);
                let uri = read_uri(data);
                let uri_str = uri.as_deref().unwrap_or("");
                let kind = classify_uri(uri_str);
                let visual = match kind {
                    UriKind::Images { id: Some(id) } | UriKind::Videos { id: Some(id) } | UriKind::Files { id: Some(id) } => {
                        self.service.visual.read().unwrap().iter().find(|v| v.id == id).cloned()
                    }
                    _ => None,
                };
                let mime = mime_for_kind(kind, visual.as_ref());
                let callback_binder: Option<SIBinder> = data.read().ok();
                if let Some(binder) = callback_binder {
                    if let Some(proxy) = binder.as_proxy() {
                        if let Ok(mut d) = proxy.prepare_transact(false) {
                            let _ = d.write_interface_token("android.os.IRemoteCallback");
                            let _ = d.write_i32(1); // non-null bundle
                            let _ = crate::bundle::write_string_bundle(&mut d, &[("result", Some(mime))]);
                            let _ = proxy.submit_transact(1, &d, rsbinder::FLAG_ONEWAY);
                        }
                    }
                }
                Ok(true)
            }
            "CANONICALIZE_ASYNC" | "UNCANONICALIZE_ASYNC" => {
                skip_attribution(data);
                let _uri = read_uri(data);
                let callback_binder: Option<SIBinder> = data.read().ok();
                if let Some(binder) = callback_binder {
                    if let Some(proxy) = binder.as_proxy() {
                        if let Ok(mut d) = proxy.prepare_transact(false) {
                            let _ = d.write_interface_token("android.os.IRemoteCallback");
                            let _ = d.write_i32(1); // non-null bundle
                            let _ = d.write_i32(0); // empty bundle -> result is null, canonicalizeOrElse uses uri
                            let _ = proxy.submit_transact(1, &d, rsbinder::FLAG_ONEWAY);
                        }
                    }
                }
                Ok(true)
            }
            "CALL" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // empty bundle
                Ok(true)
            }
            _ => {
                log::warn!("media: unhandled {name} (code {_code})");
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}

impl MediaProvider {
    fn lookup_open(&self, kind: UriKind, id: i64) -> Option<(PathBuf, String)> {
        match kind {
            UriKind::Images { .. } | UriKind::Videos { .. } | UriKind::Files { .. } => {
                let visual = self.service.visual.read().unwrap();
                visual.iter().find(|v| v.id == id).map(|v| (v.path.clone(), v.mime_type.clone()))
            }
            UriKind::Audio { .. } | UriKind::Unknown { .. } => {
                let items = self.service.items.read().unwrap();
                if let Some(a) = items.iter().find(|i| i.id == id) {
                    return Some((a.path.clone(), "audio/ogg".into()));
                }
                let visual = self.service.visual.read().unwrap();
                visual.iter().find(|v| v.id == id).map(|v| (v.path.clone(), v.mime_type.clone()))
            }
            UriKind::Thumbnails => None,
        }
    }

    fn query_visual(
        &self,
        kind: UriKind,
        id: Option<i64>,
        uri: &str,
        projection: Vec<String>,
        bundle_bytes: &[u8],
        reply: &mut Parcel,
    ) -> Result<bool> {
        let filter = parse_query_filter(uri, bundle_bytes, id);
        let type_restrict = match kind {
            UriKind::Images { .. } => Some(MEDIA_TYPE_IMAGE),
            UriKind::Videos { .. } => Some(MEDIA_TYPE_VIDEO),
            _ => None,
        };

        let guard = self.service.visual.read().unwrap();
        let mut matching: Vec<&VisualItem> = guard
            .iter()
            .filter(|item| matches_visual(item, &filter, type_restrict))
            .collect();
        matching.sort_by(|a, b| b.date_taken.cmp(&a.date_taken).then(b.id.cmp(&a.id)));

        let cols = if projection.is_empty() {
            default_visual_cols(kind)
        } else {
            projection
        };

        if is_count_only(&cols) {
            let count = matching.len() as i64;
            let mut window = CursorWindowBuilder::new("media_window", cols.clone());
            window.add_row(cols.iter().map(|_| CellValue::Integer(count)).collect());
            log::info!("media: QUERY visual count={} uri={uri}", count);
            return write_query_reply(reply, &self.bulk_cursor, &cols, 1, window);
        }

        if filter.offset > 0 {
            matching = matching.into_iter().skip(filter.offset).collect();
        }
        if let Some(lim) = filter.limit {
            matching.truncate(lim);
        }

        log::info!(
            "media: QUERY visual kind={kind:?} n={} bucket={:?} type={type_restrict:?}",
            matching.len(),
            filter.bucket_id
        );

        let mut window = CursorWindowBuilder::new("media_window", cols.clone());
        for item in &matching {
            let row = cols.iter().map(|c| visual_cell(item, c)).collect();
            window.add_row(row);
        }
        write_query_reply(reply, &self.bulk_cursor, &cols, matching.len(), window)
    }

    fn query_audio(
        &self,
        specific_id: Option<i64>,
        is_external: bool,
        _uri: &str,
        projection: Vec<String>,
        bundle_bytes: &[u8],
        reply: &mut Parcel,
    ) -> Result<bool> {
        let filter_str = String::from_utf8_lossy(bundle_bytes);
        let args = query_arg_map(bundle_bytes);
        let selection = args
            .get("android:query-arg-sql-selection")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let filter_haystack = format!("{filter_str} {selection}");

        let items_guard = self.service.items.read().unwrap();
        let matching_items: Vec<AudioItem> = if let Some(id) = specific_id {
            items_guard.iter().filter(|i| i.id == id).cloned().collect()
        } else if is_external {
            // External sounds: only user alarms if any
            items_guard.iter().filter(|i| i.container_path.starts_with("/sdcard/")).cloned().collect()
        } else if filter_haystack.contains("is_ringtone") {
            items_guard.iter().filter(|i| i.is_ringtone).cloned().collect()
        } else if filter_haystack.contains("is_notification") {
            items_guard.iter().filter(|i| i.is_notification).cloned().collect()
        } else {
            // Default to alarms (covers is_alarm and STREAM_ALARM queries)
            items_guard.iter().filter(|i| i.is_alarm).cloned().collect()
        };

        let cols = if projection.is_empty() {
            vec!["_id".to_string(), "title".to_string(), "title".to_string(), "title_key".to_string()]
        } else {
            projection
        };

        let mut window = CursorWindowBuilder::new("media_window", cols.clone());
        for item in &matching_items {
            let mut row = Vec::new();
            for col in &cols {
                match col.as_str() {
                    "_id" => row.push(CellValue::Integer(item.id)),
                    "title" | "title_key" => row.push(CellValue::String(item.title.clone())),
                    "_display_name" => row.push(CellValue::String(item.display_name.clone())),
                    "_data" => row.push(CellValue::String(item.container_path.clone())),
                    "is_alarm" => row.push(CellValue::Integer(if item.is_alarm { 1 } else { 0 })),
                    "is_ringtone" => row.push(CellValue::Integer(if item.is_ringtone { 1 } else { 0 })),
                    "is_notification" => row.push(CellValue::Integer(if item.is_notification { 1 } else { 0 })),
                    "media_type" => row.push(CellValue::Integer(MEDIA_TYPE_AUDIO as i64)),
                    "mime_type" => row.push(CellValue::String("audio/ogg".into())),
                    _ => row.push(CellValue::Null),
                }
            }
            window.add_row(row);
        }

        write_query_reply(reply, &self.bulk_cursor, &cols, matching_items.len(), window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_hash_matches_known_values() {
        assert_eq!(java_hash_code("hello"), 99162322);
        assert_eq!(java_hash_code("/storage/emulated/0/pictures"), -1617409521);
    }

    #[test]
    fn bucket_id_is_java_hash_of_lowercase_parent() {
        let (id, name) = bucket_of("/storage/emulated/0/Pictures/foo.png");
        assert_eq!(name, "Pictures");
        assert_eq!(id, java_hash_code("/storage/emulated/0/pictures"));
    }

    #[test]
    fn classify_gallery_uris() {
        assert!(matches!(
            classify_uri("content://media/external/images/media"),
            UriKind::Images { id: None }
        ));
        assert!(matches!(
            classify_uri("content://media/external/images/media/42"),
            UriKind::Images { id: Some(42) }
        ));
        assert!(matches!(
            classify_uri("content://media/external/video/media"),
            UriKind::Videos { id: None }
        ));
        assert!(matches!(
            classify_uri("content://media/external/file"),
            UriKind::Files { id: None }
        ));
        assert!(matches!(
            classify_uri("content://media/external/file?limit=0,32"),
            UriKind::Files { id: None }
        ));
        assert!(matches!(
            classify_uri("content://media/internal/audio/media"),
            UriKind::Audio { id: None, external: false }
        ));
        assert!(matches!(classify_uri("content://media/external/images/thumbnails"), UriKind::Thumbnails));
    }

    #[test]
    fn selection_parses_bucket_and_between() {
        let mut f = QueryFilter::default();
        apply_selection(&mut f, "bucket_id = ?", &["-1617409521".into()]);
        assert_eq!(f.bucket_id, Some(-1617409521));

        let mut f = QueryFilter::default();
        apply_selection(&mut f, "_id BETWEEN ? AND ?", &["10".into(), "20".into()]);
        assert_eq!(f.id_min, Some(10));
        assert_eq!(f.id_max, Some(20));

        let mut f = QueryFilter::default();
        apply_selection(
            &mut f,
            "bucket_id=? AND media_type = 1 AND _id BETWEEN ? AND ?",
            &["99".into(), "1".into(), "8".into()],
        );
        assert_eq!(f.bucket_id, Some(99));
        assert_eq!(f.media_type, Some(1));
        assert_eq!(f.id_min, Some(1));
        assert_eq!(f.id_max, Some(8));
    }

    #[test]
    fn uri_limit_is_offset_count() {
        let f = parse_query_filter("content://media/external/images/media?limit=3,5&bucketId=9", &[], None);
        assert_eq!(f.offset, 3);
        assert_eq!(f.limit, Some(5));
        assert_eq!(f.bucket_id, Some(9));
    }

    #[test]
    fn png_header_dimensions() {
        // Minimal IHDR prefix: our parser only reads the first 24 bytes.
        let dir = std::env::temp_dir().join("aro-media-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tiny.png");
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0, 0, 0, 13]); // length
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&1920u32.to_be_bytes());
        bytes.extend_from_slice(&1080u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(image_dimensions(&path), (1920, 1080));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn count_projection_detected() {
        assert!(is_count_col("COUNT(_id)"));
        assert!(is_count_col("count(*)"));
        assert!(is_count_only(&["COUNT(_id)".into()]));
        assert!(!is_count_only(&["_id".into(), "COUNT(_id)".into()]));
    }

    #[test]
    fn mime_for_common_exts() {
        assert_eq!(mime_for_ext("png"), Some(("image/png", MEDIA_TYPE_IMAGE)));
        assert_eq!(mime_for_ext("JPG"), None); // caller lowercases
        assert_eq!(mime_for_ext("jpg"), Some(("image/jpeg", MEDIA_TYPE_IMAGE)));
        assert_eq!(mime_for_ext("mp4"), Some(("video/mp4", MEDIA_TYPE_VIDEO)));
        assert_eq!(mime_for_ext("ogg"), None);
    }

    #[test]
    fn indexes_host_pictures_when_present() {
        let home = match std::env::var("HOME") {
            Ok(h) => PathBuf::from(h),
            Err(_) => return,
        };
        if !home.join("Pictures").is_dir() {
            return;
        }
        let mut next_id = 1i64;
        let items = index_visual_media(&mut next_id);
        let images = items.iter().filter(|i| i.media_type == MEDIA_TYPE_IMAGE).count();
        assert!(
            images > 0,
            "expected to index images from ~/Pictures, got {} items",
            items.len()
        );
        assert!(items.iter().all(|i| i.container_path.starts_with("/storage/emulated/0/")));
        assert!(items.iter().all(|i| i.bucket_id != 0 || i.bucket_display_name.is_empty()));
    }
}
