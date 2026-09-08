//! Filesystem detection and extraction for Android partition payloads.
//!
//! ext4 is read through `debugfs` from e2fsprogs, which walks the image as an
//! unprivileged user and recreates the tree (`rdump`). erofs (uncompressed, as
//! APEX payloads use it) is read with the pure-Rust `erofs-rs` crate.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ext4,
    Erofs,
    Unknown,
}

pub fn detect(f: &mut File) -> Result<Kind> {
    let mut sb = [0u8; 64];
    f.seek(SeekFrom::Start(1024))?;
    if f.read(&mut sb)? < 64 {
        return Ok(Kind::Unknown);
    }
    if u16::from_le_bytes([sb[56], sb[57]]) == 0xEF53 {
        return Ok(Kind::Ext4);
    }
    if u32::from_le_bytes([sb[0], sb[1], sb[2], sb[3]]) == 0xE0F5_E1E2 {
        return Ok(Kind::Erofs);
    }
    Ok(Kind::Unknown)
}

/// Extract the whole tree of an ext4 image into `out` (created if needed).
pub fn extract_ext4(image: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let out_s = out.to_str().context("output path must be UTF-8")?;
    let status = Command::new("debugfs")
        .arg("-R")
        .arg(format!("rdump / {}", shell_quote(out_s)))
        .arg(image)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null()) // ownership warnings for non-root
        .status()
        .context("running debugfs (install e2fsprogs)")?;
    if !status.success() {
        bail!("debugfs rdump failed for {}", image.display());
    }
    Ok(())
}

pub fn extract(image: &Path, out: &Path) -> Result<Kind> {
    let mut f = File::open(image)?;
    let kind = detect(&mut f)?;
    match kind {
        Kind::Ext4 => extract_ext4(image, out)?,
        Kind::Erofs => extract_erofs(image, out)?,
        Kind::Unknown => bail!("{} is not ext4 or erofs", image.display()),
    }
    Ok(kind)
}

fn shell_quote(s: &str) -> String {
    // debugfs splits its command line on whitespace; quote defensively.
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}

/// Extract an erofs image, compressed or not (APEX payloads use LZ4 clusters).
pub fn extract_erofs(image: &Path, out: &Path) -> Result<()> {
    use fs_core::{BlockRead, FileDevice};
    use fs_erofs::{FileType, Filesystem, Inode};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    let dev = Arc::new(FileDevice::open(image).map_err(|e| anyhow::anyhow!("open {}: {e:?}", image.display()))?) as Arc<dyn BlockRead>;
    let fs = Filesystem::open(dev).map_err(|e| anyhow::anyhow!("parse erofs {}: {e:?}", image.display()))?;

    fn walk(fs: &Filesystem, dir: &Inode, dest: &Path) -> Result<()> {
        std::fs::create_dir_all(dest)?;
        for entry in fs.read_dir(dir).map_err(|e| anyhow::anyhow!("read_dir: {e:?}"))? {
            let name = String::from_utf8_lossy(&entry.name).into_owned();
            if name == "." || name == ".." {
                continue;
            }
            let inode = fs.read_inode(entry.nid).map_err(|e| anyhow::anyhow!("inode {}: {e:?}", entry.nid))?;
            let target = dest.join(&name);
            let mode = (inode.mode as u32) & 0o7777;
            match inode.file_type() {
                FileType::Dir => {
                    walk(fs, &inode, &target)?;
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode | 0o700))?;
                }
                FileType::Symlink => {
                    let link = fs.read_symlink_target(&inode).map_err(|e| anyhow::anyhow!("symlink {name}: {e:?}"))?;
                    let _ = std::fs::remove_file(&target);
                    std::os::unix::fs::symlink(String::from_utf8_lossy(&link).as_ref(), &target)?;
                }
                FileType::RegularFile => {
                    let mut buf = vec![0u8; inode.size as usize];
                    if let Err(e) = fs.read_file(&inode, 0, &mut buf) {
                        // Decoder gaps on individual files (seen: crosvm in com.android.virt) must not lose the module.
                        eprintln!("    skipped {}: {e:?}", target.display());
                        continue;
                    }
                    std::fs::write(&target, &buf)?;
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode | 0o600))?;
                }
                _ => {} // device nodes, fifos: not needed from an image
            }
        }
        Ok(())
    }

    let root = fs.root_inode().map_err(|e| anyhow::anyhow!("root inode: {e:?}"))?;
    walk(&fs, &root, out)
}
