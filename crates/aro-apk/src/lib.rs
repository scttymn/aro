//! APK inspection for ARO: what PackageManager learns from AndroidManifest.xml.
pub mod axml;

use anyhow::{Context, Result};
use std::path::Path;

/// android:* attribute resource ids (frameworks/base/core/res/res/values/public.xml).
pub mod attr {
    pub const THEME: u32 = 0x0101_0000;
    pub const LABEL: u32 = 0x0101_0001;
    pub const ICON: u32 = 0x0101_0002;
    pub const NAME: u32 = 0x0101_0003;
    pub const MIME_TYPE: u32 = 0x0101_0026;
    pub const SCHEME: u32 = 0x0101_0027;
    pub const HOST: u32 = 0x0101_0028;
    pub const DEBUGGABLE: u32 = 0x0101_000f;
    pub const EXPORTED: u32 = 0x0101_0010;
    pub const PROCESS: u32 = 0x0101_0011;
    pub const TASK_AFFINITY: u32 = 0x0101_0202;
    pub const LAUNCH_MODE: u32 = 0x0101_001d;
    pub const SCREEN_ORIENTATION: u32 = 0x0101_001e;
    pub const CONFIG_CHANGES: u32 = 0x0101_001f;
    pub const MIN_SDK_VERSION: u32 = 0x0101_020c;
    pub const TARGET_SDK_VERSION: u32 = 0x0101_0270;
    pub const VERSION_CODE: u32 = 0x0101_021b;
    pub const VERSION_NAME: u32 = 0x0101_021c;
    pub const WINDOW_SOFT_INPUT_MODE: u32 = 0x0101_022b;
    pub const EXTRACT_NATIVE_LIBS: u32 = 0x0101_04ea;
    pub const APP_COMPONENT_FACTORY: u32 = 0x0101_057a;
    pub const RESIZEABLE_ACTIVITY: u32 = 0x0101_04f6;
}

#[derive(Clone, Debug, Default)]
pub struct IntentFilter {
    pub actions: Vec<String>,
    pub categories: Vec<String>,
    pub schemes: Vec<String>,
    pub hosts: Vec<String>,
    pub mime_types: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ActivityDecl {
    pub name: String,
    pub theme: u32,
    pub launch_mode: i32,
    pub exported: Option<bool>,
    pub config_changes: i32,
    pub screen_orientation: i32,
    pub soft_input_mode: i32,
    pub task_affinity: Option<String>,
    pub launcher: bool,
    pub filters: Vec<IntentFilter>,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub package: String,
    pub version_code: i32,
    pub version_name: Option<String>,
    pub min_sdk: i32,
    pub target_sdk: i32,
    pub app_class: Option<String>,
    pub app_theme: u32,
    pub app_label_res: u32,
    pub app_icon_res: u32,
    pub debuggable: bool,
    pub extract_native_libs: bool,
    pub app_component_factory: Option<String>,
    pub activities: Vec<ActivityDecl>,
}

impl Manifest {
    pub fn main_activity(&self) -> Option<&ActivityDecl> {
        self.activities.iter().find(|a| a.launcher)
    }
    pub fn activity(&self, name: &str) -> Option<&ActivityDecl> {
        self.activities.iter().find(|a| a.name == name)
    }
}

/// Expand a relative component name (".Foo") to a fully qualified one.
fn qualify(package: &str, name: &str) -> String {
    if name.starts_with('.') {
        format!("{package}{name}")
    } else if !name.contains('.') {
        format!("{package}.{name}")
    } else {
        name.to_string()
    }
}

pub fn parse_manifest(data: &[u8]) -> Result<Manifest> {
    let root = axml::parse(data)?;
    if root.name != "manifest" {
        anyhow::bail!("root element is {}, expected manifest", root.name);
    }
    let mut m = Manifest::default();
    m.package = root.attr(0, "package").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    m.version_code = root.attr(attr::VERSION_CODE, "versionCode").and_then(|v| v.as_int()).unwrap_or(1);
    m.version_name = root.attr(attr::VERSION_NAME, "versionName").and_then(|v| v.as_str()).map(String::from);
    if let Some(sdk) = root.child("uses-sdk") {
        m.min_sdk = sdk.attr(attr::MIN_SDK_VERSION, "minSdkVersion").and_then(|v| v.as_int()).unwrap_or(1);
        m.target_sdk = sdk.attr(attr::TARGET_SDK_VERSION, "targetSdkVersion").and_then(|v| v.as_int()).unwrap_or(m.min_sdk);
    }
    if let Some(app) = root.child("application") {
        m.app_class = app.attr(attr::NAME, "name").and_then(|v| v.as_str()).map(|n| qualify(&m.package, n));
        m.app_theme = app.attr(attr::THEME, "theme").and_then(|v| v.as_int()).unwrap_or(0) as u32;
        m.app_label_res = app.attr(attr::LABEL, "label").and_then(|v| v.as_int()).unwrap_or(0) as u32;
        m.app_icon_res = app.attr(attr::ICON, "icon").and_then(|v| v.as_int()).unwrap_or(0) as u32;
        m.debuggable = app.attr(attr::DEBUGGABLE, "debuggable").and_then(|v| v.as_bool()).unwrap_or(false);
        m.extract_native_libs = app.attr(attr::EXTRACT_NATIVE_LIBS, "extractNativeLibs").and_then(|v| v.as_bool()).unwrap_or(true);
        m.app_component_factory = app.attr(attr::APP_COMPONENT_FACTORY, "appComponentFactory").and_then(|v| v.as_str()).map(String::from);
        for a in app.children_named("activity").chain(app.children_named("activity-alias")) {
            let Some(name) = a.attr(attr::NAME, "name").and_then(|v| v.as_str()) else { continue };
            let names = |f: &crate::axml::Element, tag: &str| -> Vec<String> {
                f.children_named(tag).filter_map(|x| x.attr(attr::NAME, "name").and_then(|v| v.as_str()).map(String::from)).collect()
            };
            let filters: Vec<IntentFilter> = a.children_named("intent-filter").map(|f| IntentFilter {
                actions: names(f, "action"),
                categories: names(f, "category"),
                schemes: f.children_named("data").filter_map(|d| d.attr(attr::SCHEME, "scheme").and_then(|v| v.as_str()).map(String::from)).collect(),
                hosts: f.children_named("data").filter_map(|d| d.attr(attr::HOST, "host").and_then(|v| v.as_str()).map(String::from)).collect(),
                mime_types: f.children_named("data").filter_map(|d| d.attr(attr::MIME_TYPE, "mimeType").and_then(|v| v.as_str()).map(String::from)).collect(),
            }).collect();
            let launcher = filters.iter().any(|f| f.actions.iter().any(|x| x == "android.intent.action.MAIN") && f.categories.iter().any(|x| x == "android.intent.category.LAUNCHER"));
            m.activities.push(ActivityDecl {
                name: qualify(&m.package, name),
                theme: a.attr(attr::THEME, "theme").and_then(|v| v.as_int()).unwrap_or(0) as u32,
                launch_mode: a.attr(attr::LAUNCH_MODE, "launchMode").and_then(|v| v.as_int()).unwrap_or(0),
                exported: a.attr(attr::EXPORTED, "exported").and_then(|v| v.as_bool()),
                config_changes: a.attr(attr::CONFIG_CHANGES, "configChanges").and_then(|v| v.as_int()).unwrap_or(0),
                screen_orientation: a.attr(attr::SCREEN_ORIENTATION, "screenOrientation").and_then(|v| v.as_int()).unwrap_or(-1),
                soft_input_mode: a.attr(attr::WINDOW_SOFT_INPUT_MODE, "windowSoftInputMode").and_then(|v| v.as_int()).unwrap_or(0),
                task_affinity: a.attr(attr::TASK_AFFINITY, "taskAffinity").and_then(|v| v.as_str()).map(String::from),
                launcher,
                filters,
            });
        }
    }
    Ok(m)
}

/// Read and parse the manifest inside an APK.
pub fn inspect(apk: &Path) -> Result<Manifest> {
    let file = std::fs::File::open(apk).with_context(|| format!("opening {}", apk.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("reading APK zip")?;
    let mut entry = zip.by_name("AndroidManifest.xml").context("APK has no AndroidManifest.xml")?;
    let mut data = Vec::with_capacity(entry.size() as usize);
    std::io::Read::read_to_end(&mut entry, &mut data)?;
    parse_manifest(&data)
}
