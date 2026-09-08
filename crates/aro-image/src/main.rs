//! aro-image: unpack a published AOSP system image into a usable /system tree.
//!
//! Input: a GSI zip (containing `system.img`) or a bare `system.img`.
//! Output: `<out>/` holding the image's root filesystem, with every APEX in
//! `system/apex` flattened into `<out>/apex/<name>` the way Android mounts them.

mod apex;
mod fs;
mod sparse;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs::File;
use std::io::copy;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "aro-image", about = "Unpack AOSP system images for ARO")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Unpack a GSI zip or system.img into a directory
    Unpack {
        /// GSI zip or system.img
        input: PathBuf,
        /// Output directory (default: ~/.local/share/aro/system)
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Scratch directory for intermediate files (default: <out>.tmp)
        #[arg(long)]
        tmp: Option<PathBuf>,
    },
    /// Print what kind of image a file is
    Inspect { input: PathBuf },
    /// Flatten APEX modules that are missing from <root>/apex (idempotent)
    Flatten {
        /// Unpacked image root (default: ~/.local/share/aro/system)
        #[arg(short, long)]
        root: Option<PathBuf>,
    },
}

fn default_out() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    base.join("aro/system")
}

/// Obtain a raw (non-sparse) system.img from `input`, materialised under `tmp`.
fn raw_system_image(input: &Path, tmp: &Path) -> Result<PathBuf> {
    let is_zip = input.extension().map(|e| e == "zip").unwrap_or(false);
    let img = if is_zip {
        let file = File::open(input)?;
        let mut zip = zip::ZipArchive::new(file).context("opening zip")?;
        let dest = tmp.join("system.img");
        let mut e = zip.by_name("system.img").context("zip has no system.img")?;
        eprintln!("extracting system.img ({} MiB)", e.size() >> 20);
        let mut out = File::create(&dest)?;
        copy(&mut e, &mut out)?;
        dest
    } else {
        input.to_path_buf()
    };
    let mut f = File::open(&img)?;
    if sparse::is_sparse(&mut f)? {
        let raw = tmp.join("system.raw");
        eprintln!("expanding sparse image");
        let mut out = File::create(&raw)?;
        let n = sparse::expand(&mut f, &mut out)?;
        eprintln!("raw image: {} MiB", n >> 20);
        return Ok(raw);
    }
    Ok(img)
}

fn unpack(input: &Path, out: &Path, tmp: &Path) -> Result<()> {
    if out.exists() && std::fs::read_dir(out)?.next().is_some() {
        bail!("{} exists and is not empty; remove it first", out.display());
    }
    std::fs::create_dir_all(tmp)?;
    let raw = raw_system_image(input, tmp)?;
    eprintln!("extracting root filesystem to {}", out.display());
    let kind = fs::extract(&raw, out)?;
    eprintln!("root filesystem: {kind:?}");
    let apex_dir = out.join("system/apex");
    if apex_dir.is_dir() {
        eprintln!("flattening APEX modules");
        let names = apex::flatten_all(&apex_dir, out, tmp)?;
        eprintln!("{} APEX modules flattened", names.len());
    }
    std::fs::remove_dir_all(tmp).ok();
    // Sanity: the pieces M1 needs.
    for p in ["system/bin/app_process64", "system/bin/linker64", "system/framework/framework.jar", "apex/com.android.art/lib64/libart.so", "apex/com.android.art/javalib/core-oj.jar"] {
        let ok = out.join(p).exists();
        eprintln!("  {} {}", if ok { "ok " } else { "MISSING" }, p);
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Unpack { input, out, tmp } => {
            let out = out.unwrap_or_else(default_out);
            let tmp = tmp.unwrap_or_else(|| PathBuf::from(format!("{}.tmp", out.display())));
            unpack(&input, &out, &tmp)
        }
        Cmd::Flatten { root } => {
            let root = root.unwrap_or_else(default_out);
            let tmp = PathBuf::from(format!("{}.tmp", root.display()));
            std::fs::create_dir_all(&tmp)?;
            let names = apex::flatten_missing(&root.join("system/apex"), &root, &tmp)?;
            std::fs::remove_dir_all(&tmp).ok();
            eprintln!("{} APEX modules flattened: {}", names.len(), names.join(" "));
            Ok(())
        }
        Cmd::Inspect { input } => {
            let mut f = File::open(&input)?;
            if sparse::is_sparse(&mut f)? {
                println!("android sparse image");
            } else {
                println!("{:?}", fs::detect(&mut f)?);
            }
            Ok(())
        }
    }
}
