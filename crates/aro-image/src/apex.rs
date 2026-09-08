//! APEX flattening.
//!
//! An `.apex` is a zip containing `apex_payload.img` (a filesystem image) and
//! `apex_manifest.pb`. A `.capex` is a zip whose `original_apex` entry is the
//! `.apex`. At runtime Android mounts each payload at `/apex/<name>`; we
//! extract them into that layout so the unpacked tree is usable directly.

use crate::fs;
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{copy, Read};
use std::path::{Path, PathBuf};

/// Extract `apex_payload.img` out of an `.apex`/`.capex` into `tmp`, returning its path.
pub fn payload_image(apex: &Path, tmp: &Path) -> Result<PathBuf> {
    let file = File::open(apex)?;
    let mut zip = zip::ZipArchive::new(file).with_context(|| format!("opening {}", apex.display()))?;
    if zip.by_name("original_apex").is_ok() {
        // Compressed APEX: unwrap the inner archive first.
        let inner = tmp.join("original_apex");
        {
            let mut e = zip.by_name("original_apex")?;
            let mut out = File::create(&inner)?;
            copy(&mut e, &mut out)?;
        }
        return payload_image(&inner, tmp);
    }
    let mut e = zip.by_name("apex_payload.img").context("apex without apex_payload.img")?;
    let img = tmp.join("apex_payload.img");
    let mut out = File::create(&img)?;
    copy(&mut e, &mut out)?;
    Ok(img)
}

/// APEX module name from the file name (`com.android.art.capex` -> `com.android.art`).
pub fn module_name(apex: &Path) -> String {
    let stem = apex.file_name().and_then(|s| s.to_str()).unwrap_or("");
    stem.trim_end_matches(".capex").trim_end_matches(".apex").to_string()
}

/// Flatten every APEX under `apex_dir` into `root/apex/<name>`.
pub fn flatten_all(apex_dir: &Path, root: &Path, tmp: &Path) -> Result<Vec<String>> {
    flatten(apex_dir, root, tmp, false)
}

/// Flatten only the APEXes whose `root/apex/<name>` does not exist yet.
pub fn flatten_missing(apex_dir: &Path, root: &Path, tmp: &Path) -> Result<Vec<String>> {
    flatten(apex_dir, root, tmp, true)
}

fn flatten(apex_dir: &Path, root: &Path, tmp: &Path, only_missing: bool) -> Result<Vec<String>> {
    let mut done = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(apex_dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = module_name(&path);
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "apex" && ext != "capex" {
            continue;
        }
        if only_missing && root.join("apex").join(&name).is_dir() {
            continue;
        }
        let work = tmp.join(&name);
        std::fs::create_dir_all(&work)?;
        let img = payload_image(&path, &work)?;
        let mut f = File::open(&img)?;
        let mut raw = img.clone();
        if crate::sparse::is_sparse(&mut f)? {
            raw = work.join("payload.raw");
            let mut out = File::create(&raw)?;
            crate::sparse::expand(&mut f, &mut out)?;
        }
        let dest = root.join("apex").join(&name);
        let kind = match fs::extract(&raw, &dest) {
            Ok(k) => k,
            Err(e) => {
                // e.g. erofs payloads (com.android.virt): skip, keep going.
                eprintln!("  apex {name}: SKIPPED ({e:#})");
                std::fs::remove_dir_all(&dest).ok();
                std::fs::remove_dir_all(&work).ok();
                continue;
            }
        };
        eprintln!("  apex {name}: {kind:?}");
        // Keep the manifest next to the payload for later inspection.
        let _ = copy_entry(&path, "apex_manifest.pb", &dest.join("apex_manifest.pb"));
        std::fs::remove_dir_all(&work).ok();
        done.push(name);
    }
    Ok(done)
}

fn copy_entry(apex: &Path, entry: &str, dest: &Path) -> Result<()> {
    let file = File::open(apex)?;
    let mut zip = zip::ZipArchive::new(file)?;
    if zip.by_name("original_apex").is_ok() {
        let mut inner = Vec::new();
        zip.by_name("original_apex")?.read_to_end(&mut inner)?;
        let mut z2 = zip::ZipArchive::new(std::io::Cursor::new(inner))?;
        let mut e = z2.by_name(entry)?;
        let mut out = File::create(dest)?;
        copy(&mut e, &mut out)?;
        return Ok(());
    }
    let mut e = zip.by_name(entry)?;
    let mut out = File::create(dest)?;
    copy(&mut e, &mut out)?;
    Ok(())
}
