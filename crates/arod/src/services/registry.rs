//! The session's package registry: what ARO knows about installed apps.
//! Until the APK parser lands, entries come from the launch command line.
use crate::aparcel as ap;
use crate::parcelables::{ActivityInfo, ApplicationInfo, SharedLibraryInfo};
use rsbinder::{Parcel, Result};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct AppSpec {
    pub package: String,
    pub apk_in_ns: String,
    pub app_class: Option<String>,
    pub main_activity: Option<String>,
    pub target_sdk: i32,
    pub min_sdk: i32,
    pub uid: i32,
    pub version_code: i32,
    pub version_name: String,
    pub app_theme: u32,
    pub label_res: u32,
    pub icon_res: u32,
    pub app_component_factory: Option<String>,
    pub activities: Vec<aro_apk::ActivityDecl>,
}

impl AppSpec {
    /// Build from a parsed manifest; `apk_in_ns` is the APK path as the app sees it.
    pub fn from_manifest(m: &aro_apk::Manifest, apk_in_ns: String, uid: i32) -> Self {
        AppSpec {
            package: m.package.clone(),
            apk_in_ns,
            app_class: m.app_class.clone(),
            main_activity: m.main_activity().map(|a| a.name.clone()),
            target_sdk: m.target_sdk,
            min_sdk: m.min_sdk,
            uid,
            version_code: m.version_code,
            version_name: m.version_name.clone().unwrap_or_else(|| "1.0".into()),
            app_theme: m.app_theme,
            label_res: m.app_label_res,
            icon_res: m.app_icon_res,
            app_component_factory: m.app_component_factory.clone(),
            activities: m.activities.clone(),
        }
    }
}

pub struct Registry {
    pub apps: Mutex<Vec<AppSpec>>,
    /// Display the session presents: width, height, dpi.
    pub display: (i32, i32, i32),
}

impl Registry {
    pub fn find(&self, package: &str) -> Option<AppSpec> {
        self.apps.lock().unwrap().iter().find(|a| a.package == package).cloned()
    }

    pub fn find_uid(&self, uid: i32) -> Option<AppSpec> {
        self.apps.lock().unwrap().iter().find(|a| a.uid == uid).cloned()
    }

    pub fn application_info(spec: &AppSpec) -> ApplicationInfo {
        let mut ai = ApplicationInfo::default();
        ai.base.package_name = Some(spec.package.clone());
        ai.base.name = spec.app_class.clone();
        ai.class_name = spec.app_class.clone();
        ai.process_name = Some(spec.package.clone());
        ai.task_affinity = Some(spec.package.clone());
        ai.source_dir = Some(spec.apk_in_ns.clone());
        ai.public_source_dir = Some(spec.apk_in_ns.clone());
        ai.native_library_dir = Some(format!("/data/app/{}/lib/x86_64", spec.package));
        ai.data_dir = Some(format!("/data/user/0/{}", spec.package));
        ai.credential_protected_data_dir = ai.data_dir.clone();
        ai.device_protected_data_dir = Some(format!("/data/user_de/0/{}", spec.package));
        ai.target_sdk_version = spec.target_sdk;
        ai.min_sdk_version = spec.min_sdk;
        ai.theme = spec.app_theme as i32;
        ai.base.label_res = spec.label_res as i32;
        ai.base.icon = spec.icon_res as i32;
        ai.app_component_factory = spec.app_component_factory.clone();
        ai.uid = spec.uid;
        ai.long_version_code = spec.version_code as i64;
        // Window extensions: a real device links these into every app when extensions are enabled.
        ai.shared_library_infos = vec![
            SharedLibraryInfo::builtin("/system_ext/framework/androidx.window.extensions.jar", "androidx.window.extensions"),
            SharedLibraryInfo::builtin("/system_ext/framework/androidx.window.sidecar.jar", "androidx.window.sidecar"),
        ];
        ai
    }

    /// android.content.pm.ActivityInfo for one of the app's declared activities.
    pub fn activity_info(spec: &AppSpec, name: &str) -> Option<ActivityInfo> {
        let decl = spec.activities.iter().find(|a| a.name == name)?;
        Some(ActivityInfo {
            name: decl.name.clone(),
            package_name: spec.package.clone(),
            application_info: Self::application_info(spec),
            process_name: spec.package.clone(),
            theme: if decl.theme != 0 { decl.theme as i32 } else { spec.app_theme as i32 },
            launch_mode: decl.launch_mode,
            flags: 0,
            config_changes: decl.config_changes,
            screen_orientation: decl.screen_orientation,
            soft_input_mode: decl.soft_input_mode,
            task_affinity: Some(decl.task_affinity.clone().unwrap_or_else(|| spec.package.clone())),
        })
    }

    /// android.content.pm.PackageInfo (spec/parcels/android.content.pm.PackageInfo.txt).
    pub fn write_package_info(spec: &AppSpec, p: &mut Parcel) -> Result<()> {
        ap::string8(p, Some(&spec.package))?;
        ap::null_array(p)?; // splitNames
        p.write_i32(spec.version_code)?;
        p.write_i32(0)?; // versionCodeMajor
        ap::string8(p, Some(&spec.version_name))?;
        p.write_i32(0)?; // baseRevisionCode
        ap::null_array(p)?; // splitRevisionCodes
        ap::string8(p, None)?; // sharedUserId
        p.write_i32(0)?; // sharedUserLabel
        p.write_i32(1)?; // applicationInfo present
        Self::application_info(spec).write(p)?;
        p.write_i64(0)?; // firstInstallTime
        p.write_i64(0)?; // lastUpdateTime
        ap::null_array(p)?; // gids
        ap::null_array(p)?; // activities
        ap::null_array(p)?; // receivers
        ap::null_array(p)?; // services
        ap::null_array(p)?; // providers
        ap::null_array(p)?; // instrumentation
        ap::null_array(p)?; // permissions
        ap::null_array(p)?; // requestedPermissions
        ap::null_array(p)?; // requestedPermissionsFlags
        ap::null_array(p)?; // requestedPermissionsPurposes: writeBundle(null)
        ap::null_array(p)?; // attributions
        ap::null_array(p)?; // configPreferences
        ap::null_array(p)?; // reqFeatures
        ap::null_array(p)?; // featureGroups
        ap::null_array(p)?; // signatures
        p.write_i32(0)?; // installLocation
        p.write_i32(0)?; // isStub
        p.write_i32(0)?; // coreApp
        p.write_i32(0)?; // requiredForAllUsers
        ap::string8(p, None)?; // restrictedAccountType
        ap::string8(p, None)?; // requiredAccountType
        ap::string8(p, None)?; // overlayTarget
        ap::string8(p, None)?; // overlayCategory
        p.write_i32(0)?; // overlayPriority
        ap::boolean(p, false)?; // mOverlayIsStatic
        p.write_i32(37)?; // compileSdkVersion
        ap::string8(p, None)?; // compileSdkVersionCodename
        p.write_i32(0)?; // signingInfo: null
        ap::boolean(p, false)?; // isApex
        ap::boolean(p, false)?; // isActiveApex
        p.write_i64(0)?; // mArchiveTimeMillis
        p.write_i32(0)?; // mApexPackageName: absent
        ap::boolean(p, false) // mIsAppMetadataVerified
    }
}
