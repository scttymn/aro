//! Host-side preparation of the regenerable state an Android process needs.
use crate::layout::Layout;
use anyhow::{Context, Result};
use std::path::Path;

/// `apex-info-list.xml`: what apexd writes at boot; linkerconfig and others read it.
pub fn write_apex_info_list(system: &Path) -> Result<usize> {
    let apex_dir = system.join("apex");
    let mut rows = Vec::new();
    let mut names: Vec<_> = std::fs::read_dir(&apex_dir)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let manifest = apex_dir.join(&name).join("apex_manifest.pb");
        let Ok(bytes) = std::fs::read(&manifest) else { continue };
        let version = manifest_version(&bytes).unwrap_or(0);
        let src = ["capex", "apex"]
            .iter()
            .map(|ext| format!("/system/apex/{name}.{ext}"))
            .find(|p| system.join(&p[1..]).exists())
            .unwrap_or(format!("/system/apex/{name}.apex"));
        rows.push(format!(
            "  <apex-info moduleName=\"{name}\" modulePath=\"{src}\" preinstalledModulePath=\"{src}\" versionCode=\"{version}\" versionName=\"\" isFactory=\"true\" isActive=\"true\" lastUpdateMillis=\"0\" provideSharedApexLibs=\"false\" partition=\"SYSTEM\"></apex-info>"
        ));
    }
    let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<apex-info-list>\n{}\n</apex-info-list>\n", rows.join("\n"));
    std::fs::write(apex_dir.join("apex-info-list.xml"), xml)?;
    Ok(rows.len())
}

/// Field 2 (varint `version`) of an ApexManifest protobuf.
fn manifest_version(b: &[u8]) -> Option<u64> {
    let mut i = 0;
    let varint = |i: &mut usize| -> Option<u64> {
        let (mut r, mut s) = (0u64, 0);
        loop {
            let x = *b.get(*i)?;
            *i += 1;
            r |= ((x & 0x7f) as u64) << s;
            s += 7;
            if x < 0x80 {
                return Some(r);
            }
        }
    };
    while i < b.len() {
        let key = varint(&mut i)?;
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => {
                let v = varint(&mut i)?;
                if field == 2 {
                    return Some(v);
                }
            }
            2 => {
                let len = varint(&mut i)? as usize;
                i += len;
            }
            1 => i += 8,
            5 => i += 4,
            _ => return None,
        }
    }
    None
}

/// aconfig flag storage: `/metadata/aconfig/{maps,boot}` as aconfigd lays it out.
pub fn write_aconfig(system: &Path, state: &Path) -> Result<usize> {
    let meta = state.join("metadata/aconfig");
    std::fs::create_dir_all(meta.join("maps"))?;
    std::fs::create_dir_all(meta.join("boot"))?;
    let lay = |container: &str, dir: &Path| -> Result<bool> {
        let files = [("package.map", format!("maps/{container}.package.map")), ("flag.map", format!("maps/{container}.flag.map")), ("flag.val", format!("boot/{container}.val")), ("flag.info", format!("boot/{container}.info"))];
        let mut all = true;
        for (src, dst) in files {
            let p = dir.join(src);
            if p.exists() {
                std::fs::copy(&p, meta.join(dst))?;
            } else {
                all = false;
            }
        }
        Ok(all)
    };
    let mut n = 0;
    if lay("system", &system.join("system/etc/aconfig"))? {
        n += 1;
    }
    let mut names: Vec<_> = std::fs::read_dir(system.join("apex"))?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let dir = system.join("apex").join(&name).join("etc");
        if dir.join("package.map").exists() && lay(&name, &dir)? {
            n += 1;
        }
    }
    Ok(n)
}

/// System properties: the image's build.prop plus what a device's vendor
/// partition would normally provide.
pub fn write_properties(system: &Path, state: &Path, extra: &[(String, String)]) -> Result<usize> {
    let mut props = Vec::new();
    let build_prop = std::fs::read_to_string(system.join("system/build.prop")).context("reading system/build.prop")?;
    props.extend(aro_props::parse_prop_file(&build_prop));
    for (k, v) in [
        ("ro.product.cpu.abilist", "x86_64,x86"),
        ("ro.product.cpu.abilist64", "x86_64"),
        ("ro.product.cpu.abilist32", "x86"),
        ("ro.dalvik.vm.native.bridge", "0"),
        ("ro.dalvik.vm.enable_uffd_gc", "true"),
        ("ro.zygote", "zygote64"),
        ("ro.boot.hardware", "aro"),
        ("ro.hardware", "aro"),
        ("ro.debuggable", "1"),
        ("ro.kernel.qemu", "0"),
        ("dalvik.vm.heapstartsize", "8m"),
        ("dalvik.vm.heapgrowthlimit", "256m"),
        ("dalvik.vm.heapsize", "512m"),
        ("dalvik.vm.usejit", "true"),
        ("aro.version", env!("CARGO_PKG_VERSION")),
        // Boot state Android's own daemons would set; on ARO the bus is up before any app runs.
        ("servicemanager.ready", "true"),
        // No HIDL in ARO: libhidl callers (gralloc4 fallback, etc.) fail fast instead of waiting.
        ("hwservicemanager.disabled", "true"),
        // The GSI build.prop sets this true; ARO has no hwservicemanager to *create*
        // the .disabled property at boot, so callers would block in WaitForPropertyCreation.
        // False makes libhidl skip that wait and honour the static .disabled=true above.
        ("hwservicemanager.always_sets_disabled", "false"),
        ("apexd.status", "ready"),
        ("sys.boot_completed", "1"),
        ("dev.bootcomplete", "1"),
        ("service.bootanim.exit", "1"),
        ("sys.user.0.ce_available", "true"),
        ("sys.boot.reason", "reboot"),
        ("ro.boot.verifiedbootstate", "green"),
        // Window extensions (androidx.window.extensions on system_ext) are present in the image;
        // the framework initialises them for every app when this is set, as on real devices.
        ("persist.wm.extensions.enabled", "true"),
    ] {
        props.push((k.to_string(), v.to_string()));
    }
    props.extend(extra.iter().cloned());
    // Later entries (ARO extras) override earlier ones (build.prop).
    let mut deduped: Vec<(String, String)> = Vec::new();
    for (k, v) in props.into_iter() {
        if let Some(slot) = deduped.iter_mut().find(|(ek, _)| *ek == k) {
            slot.1 = v;
        } else {
            deduped.push((k, v));
        }
    }
    let props = deduped;
    let dir = state.join("props");
    let _ = std::fs::remove_dir_all(&dir);
    aro_props::write_properties_dir(&dir, &props)
}

pub fn prepare(layout: &Layout) -> Result<()> {
    std::fs::create_dir_all(&layout.state)?;
    std::fs::create_dir_all(layout.data.join("dalvik-cache/x86_64"))?;
    std::fs::create_dir_all(layout.data.join("local/tmp"))?;
    std::fs::create_dir_all(layout.state.join("linkerconfig"))?;
    std::fs::create_dir_all(&layout.runtime)?;
    let n = write_apex_info_list(&layout.system)?;
    eprintln!("aro-exec: {n} APEX modules listed");
    let n = write_aconfig(&layout.system, &layout.state)?;
    eprintln!("aro-exec: {n} aconfig containers");
    let n = write_properties(&layout.system, &layout.state, &[])?;
    eprintln!("aro-exec: {n} system properties");
    Ok(())
}
