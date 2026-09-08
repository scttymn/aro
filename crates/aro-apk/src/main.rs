//! aro-apk: print what ARO learns from an APK's manifest.
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).ok_or_else(|| anyhow::anyhow!("usage: aro-apk <file.apk>"))?;
    let m = aro_apk::inspect(std::path::Path::new(&path))?;
    println!("package: {}  version: {} ({})", m.package, m.version_code, m.version_name.as_deref().unwrap_or("-"));
    println!("sdk: min {} target {}", m.min_sdk, m.target_sdk);
    println!("application: class={} theme=0x{:08x} label=0x{:08x} icon=0x{:08x} debuggable={}", m.app_class.as_deref().unwrap_or("-"), m.app_theme, m.app_label_res, m.app_icon_res, m.debuggable);
    for a in &m.activities {
        println!("activity: {}{} theme=0x{:08x} launchMode={} configChanges=0x{:x}", a.name, if a.launcher { " [LAUNCHER]" } else { "" }, a.theme, a.launch_mode, a.config_changes);
    }
    Ok(())
}
