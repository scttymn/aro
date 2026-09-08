//! Framework parcelables, written exactly as this image's `writeToParcel`
//! methods do (see `spec/parcels/`). Only the fields ARO sets are modelled;
//! everything else takes Android's own defaults.
use crate::aparcel as ap;
use rsbinder::{Parcel, Result};

/// android.content.pm.PackageItemInfo (base of ApplicationInfo, ActivityInfo, ...).
#[derive(Clone, Debug, Default)]
pub struct PackageItemInfo {
    pub name: Option<String>,
    pub package_name: Option<String>,
    pub label_res: i32,
    pub non_localized_label: Option<String>,
    pub icon: i32,
    pub logo: i32,
    pub banner: i32,
    pub show_user_icon: i32,
    pub is_archived: bool,
}

impl PackageItemInfo {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        ap::string8(p, self.name.as_deref())?;
        ap::string8(p, self.package_name.as_deref())?;
        p.write_i32(self.label_res)?;
        ap::char_sequence(p, self.non_localized_label.as_deref())?;
        p.write_i32(self.icon)?;
        p.write_i32(self.logo)?;
        ap::null_array(p)?; // metaData Bundle
        p.write_i32(self.banner)?;
        p.write_i32(self.show_user_icon)?;
        ap::boolean(p, self.is_archived)
    }
}

/// android.content.pm.ApplicationInfo.
#[derive(Clone, Debug)]
pub struct ApplicationInfo {
    pub base: PackageItemInfo,
    pub task_affinity: Option<String>,
    pub permission: Option<String>,
    pub process_name: Option<String>,
    pub class_name: Option<String>,
    pub theme: i32,
    pub flags: i32,
    pub private_flags: i32,
    pub private_flags_ext: i32,
    pub source_dir: Option<String>,
    pub public_source_dir: Option<String>,
    pub native_library_dir: Option<String>,
    pub primary_cpu_abi: Option<String>,
    pub data_dir: Option<String>,
    pub device_protected_data_dir: Option<String>,
    pub credential_protected_data_dir: Option<String>,
    pub uid: i32,
    pub min_sdk_version: i32,
    pub target_sdk_version: i32,
    pub long_version_code: i64,
    pub enabled: bool,
    pub compile_sdk_version: i32,
    pub app_component_factory: Option<String>,
    /// Extra jars on the app class path (what PackageManager adds for
    /// `<uses-library>` and implicit libraries such as androidx.window.extensions).
    pub shared_library_files: Vec<String>,
    /// Shared libraries loaded through their own class loaders (jars are only honoured this way).
    pub shared_library_infos: Vec<SharedLibraryInfo>,
}

/// android.content.pm.SharedLibraryInfo
#[derive(Clone, Debug)]
pub struct SharedLibraryInfo {
    pub path: String,
    pub package_name: String,
    pub name: String,
    pub version: i64,
    pub lib_type: i32,
}

pub const SHARED_LIBRARY_TYPE_BUILTIN: i32 = 0;
pub const SHARED_LIBRARY_VERSION_UNDEFINED: i64 = -1;

impl SharedLibraryInfo {
    pub fn builtin(path: &str, name: &str) -> Self {
        SharedLibraryInfo { path: path.into(), package_name: name.into(), name: name.into(), version: SHARED_LIBRARY_VERSION_UNDEFINED, lib_type: SHARED_LIBRARY_TYPE_BUILTIN }
    }
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        ap::string8(p, Some(&self.path))?; // mPath
        ap::string8(p, Some(&self.package_name))?; // mPackageName
        p.write_i32(1)?; // codePaths present
        ap::string8_array(p, Some(&[self.path.as_str()]))?;
        ap::string8(p, Some(&self.name))?; // mName
        p.write_i64(self.version)?; // mVersion
        p.write_i32(self.lib_type)?; // mType
        // mDeclaringPackage: writeParcelable(VersionedPackage)
        ap::string16(p, Some("android.content.pm.VersionedPackage"))?;
        ap::string8(p, Some(&self.package_name))?;
        p.write_i64(self.version)?;
        ap::null_array(p)?; // mDependentPackages: writeList(null)
        ap::null_array(p)?; // mDependencies: writeTypedList(null)
        ap::boolean(p, false)?; // mIsNative
        ap::null_array(p)?; // mOptionalDependentPackages: writeParcelableList(null)
        ap::null_array(p) // mCertDigests: writeStringList(null)
    }
}

pub const FLAG_HAS_CODE: i32 = 1 << 2;
pub const FLAG_ALLOW_CLEAR_USER_DATA: i32 = 1 << 6;
pub const FLAG_ALLOW_BACKUP: i32 = 1 << 15;
pub const FLAG_SUPPORTS_SCREEN_DENSITIES: i32 = (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12) | (1 << 13);
pub const FLAG_INSTALLED: i32 = 1 << 23;
pub const FLAG_HARDWARE_ACCELERATED: i32 = 1 << 29;
pub const FLAG_EXTRACT_NATIVE_LIBS: i32 = 1 << 28;
pub const FLAG_DEBUGGABLE: i32 = 1 << 1;

impl Default for ApplicationInfo {
    fn default() -> Self {
        ApplicationInfo {
            base: PackageItemInfo::default(),
            task_affinity: None,
            permission: None,
            process_name: None,
            class_name: None,
            theme: 0,
            flags: FLAG_HAS_CODE | FLAG_ALLOW_CLEAR_USER_DATA | FLAG_ALLOW_BACKUP | FLAG_SUPPORTS_SCREEN_DENSITIES | FLAG_INSTALLED | FLAG_HARDWARE_ACCELERATED | FLAG_EXTRACT_NATIVE_LIBS,
            private_flags: 0,
            private_flags_ext: 0,
            source_dir: None,
            public_source_dir: None,
            native_library_dir: None,
            primary_cpu_abi: Some("x86_64".into()),
            data_dir: None,
            device_protected_data_dir: None,
            credential_protected_data_dir: None,
            uid: 10001,
            min_sdk_version: 21,
            target_sdk_version: 37,
            long_version_code: 1,
            enabled: true,
            compile_sdk_version: 37,
            app_component_factory: None,
            shared_library_files: Vec::new(),
            shared_library_infos: Vec::new(),
        }
    }
}

impl ApplicationInfo {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        p.write_i32(0)?; // maybeWriteSquashed: not squashed
        self.base.write(p)?;
        ap::string8(p, self.task_affinity.as_deref())?;
        ap::string8(p, self.permission.as_deref())?;
        ap::string8(p, self.process_name.as_deref())?;
        ap::string8(p, self.class_name.as_deref())?;
        p.write_i32(self.theme)?;
        p.write_i32(self.flags)?;
        p.write_i32(self.private_flags)?;
        p.write_i32(self.private_flags_ext)?;
        p.write_i32(0)?; // requiresSmallestWidthDp
        p.write_i32(0)?; // compatibleWidthLimitDp
        p.write_i32(0)?; // largestWidthLimitDp
        p.write_i32(0)?; // storageUuid: null
        ap::string8(p, self.source_dir.as_deref())?; // scanSourceDir
        ap::string8(p, self.public_source_dir.as_deref())?; // scanPublicSourceDir
        ap::string8(p, self.source_dir.as_deref())?;
        ap::string8(p, self.public_source_dir.as_deref())?;
        ap::null_array(p)?; // splitNames
        ap::null_array(p)?; // splitSourceDirs
        ap::null_array(p)?; // splitPublicSourceDirs
        ap::null_array(p)?; // splitDependencies
        ap::string8(p, self.native_library_dir.as_deref())?;
        ap::string8(p, None)?; // secondaryNativeLibraryDir
        ap::string8(p, self.native_library_dir.as_deref())?; // nativeLibraryRootDir
        p.write_i32(0)?; // nativeLibraryRootRequiresIsa
        ap::string8(p, self.primary_cpu_abi.as_deref())?;
        ap::string8(p, None)?; // secondaryCpuAbi
        ap::null_array(p)?; // resourceDirs
        ap::null_array(p)?; // overlayPaths
        ap::string8(p, Some("default"))?; // seInfo
        ap::string8(p, None)?; // seInfoUser
        if self.shared_library_files.is_empty() {
            ap::null_array(p)?; // sharedLibraryFiles
        } else {
            let refs: Vec<&str> = self.shared_library_files.iter().map(String::as_str).collect();
            ap::string8_array(p, Some(&refs))?;
        }
        if self.shared_library_infos.is_empty() {
            ap::null_array(p)?; // sharedLibraryInfos
        } else {
            p.write_i32(self.shared_library_infos.len() as i32)?;
            for lib in &self.shared_library_infos {
                ap::typed(p, |p| lib.write(p))?;
            }
        }
        ap::null_array(p)?; // optionalSharedLibraryInfos
        ap::string8(p, self.data_dir.as_deref())?;
        ap::string8(p, self.device_protected_data_dir.as_deref())?;
        ap::string8(p, self.credential_protected_data_dir.as_deref())?;
        p.write_i32(self.uid)?;
        p.write_i32(-1)?; // pccUid
        p.write_i32(self.min_sdk_version)?;
        p.write_i32(self.target_sdk_version)?;
        p.write_i64(self.long_version_code)?;
        p.write_i32(self.enabled as i32)?;
        p.write_i32(0)?; // enabledSetting: COMPONENT_ENABLED_STATE_DEFAULT
        p.write_i32(0)?; // installLocation
        ap::string8(p, None)?; // manageSpaceActivityName
        ap::string8(p, None)?; // backupAgentName
        p.write_i32(0)?; // backupAgentProcess
        p.write_i32(0)?; // descriptionRes
        p.write_i32(0)?; // uiOptions
        p.write_i32(0)?; // fullBackupContent
        p.write_i32(0)?; // dataExtractionRulesRes
        ap::boolean(p, false)?; // crossProfile
        p.write_i32(0)?; // networkSecurityConfigRes
        p.write_i32(-1)?; // category: CATEGORY_UNDEFINED
        p.write_i32(0)?; // targetSandboxVersion
        ap::string8(p, None)?; // classLoaderName
        ap::null_array(p)?; // splitClassLoaderNames
        p.write_i32(self.compile_sdk_version)?;
        ap::string8(p, None)?; // compileSdkVersionCodename
        ap::string8(p, self.app_component_factory.as_deref())?;
        p.write_i32(0)?; // iconRes
        p.write_i32(0)?; // roundIconRes
        p.write_i32(0)?; // mHiddenApiPolicy: HIDDEN_API_ENFORCEMENT_DEFAULT (-1 in newer; 0 = disabled)
        p.write_i32(0)?; // hiddenUntilInstalled
        ap::string8(p, None)?; // zygotePreloadName
        ap::string8(p, None)?; // zygotePreloadNativeLib
        ap::string8(p, None)?; // zygotePreloadNativeFunc
        p.write_i32(0)?; // gwpAsanMode
        p.write_i32(0)?; // memtagMode
        p.write_i32(0)?; // nativeHeapZeroInitialized
        ap::boxed_boolean(p, None)?; // requestRawExternalStorageAccess
        p.write_i64(0)?; // createTimestamp
        p.write_i32(0)?; // mAppClassNamesByProcess: null
        p.write_i32(0)?; // localeConfigRes
        p.write_i32(0)?; // allowCrossUidActivitySwitchFromBelow
        p.write_i32(0)?; // mPageSizeAppCompatFlags
        ap::boolean(p, false)?; // isAppLockSupported
        ap::boolean(p, false)?; // isAppLockEnabled
        ap::null_array(p)?; // unalignedNativeLibraries
        ap::null_array(p)?; // ForStringSet: null
        ap::null_array(p)?; // memoryBudgets
        Ok(())
    }
}

/// android.graphics.Rect
#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        p.write_i32(self.left)?;
        p.write_i32(self.top)?;
        p.write_i32(self.right)?;
        p.write_i32(self.bottom)
    }
}

/// android.app.WindowConfiguration
#[derive(Clone, Debug)]
pub struct WindowConfiguration {
    pub bounds: Rect,
    pub app_bounds: Option<Rect>,
    pub max_bounds: Rect,
    pub windowing_mode: i32,
    pub activity_type: i32,
    pub always_on_top: i32,
    pub rotation: i32,
    pub display_rotation: i32,
}

pub const WINDOWING_MODE_FULLSCREEN: i32 = 1;
pub const ACTIVITY_TYPE_STANDARD: i32 = 1;

impl WindowConfiguration {
    pub fn fullscreen(w: i32, h: i32) -> Self {
        let r = Rect { left: 0, top: 0, right: w, bottom: h };
        WindowConfiguration { bounds: r, app_bounds: Some(r), max_bounds: r, windowing_mode: WINDOWING_MODE_FULLSCREEN, activity_type: ACTIVITY_TYPE_STANDARD, always_on_top: 0, rotation: 0, display_rotation: 0 }
    }
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        self.bounds.write(p)?;
        match &self.app_bounds {
            Some(r) => ap::typed(p, |p| r.write(p))?,
            None => ap::typed_none(p)?,
        }
        self.max_bounds.write(p)?;
        p.write_i32(self.windowing_mode)?;
        p.write_i32(self.activity_type)?;
        p.write_i32(self.always_on_top)?;
        p.write_i32(self.rotation)?;
        p.write_i32(self.display_rotation)
    }
}

/// android.content.res.Configuration
#[derive(Clone, Debug)]
pub struct Configuration {
    pub font_scale: f32,
    pub locales: String, // LocaleList string representation, e.g. "en-US"
    pub user_set_locale: bool,
    pub touchscreen: i32,
    pub keyboard: i32,
    pub keyboard_hidden: i32,
    pub hard_keyboard_hidden: i32,
    pub navigation: i32,
    pub navigation_hidden: i32,
    pub orientation: i32,
    pub screen_layout: i32,
    pub color_mode: i32,
    pub ui_mode: i32,
    pub screen_width_dp: i32,
    pub screen_height_dp: i32,
    pub smallest_screen_width_dp: i32,
    pub density_dpi: i32,
    pub window: WindowConfiguration,
    pub seq: i32,
    pub font_weight_adjustment: i32,
    pub grammatical_gender: i32,
}

impl Configuration {
    /// A landscape desktop window of `w`x`h` pixels at `dpi`.
    pub fn desktop(w: i32, h: i32, dpi: i32) -> Self {
        let wdp = w * 160 / dpi;
        let hdp = h * 160 / dpi;
        Configuration {
            font_scale: 1.0,
            locales: "en-US".into(),
            user_set_locale: false,
            touchscreen: 1,          // TOUCHSCREEN_NOTOUCH
            keyboard: 2,             // KEYBOARD_QWERTY
            keyboard_hidden: 1,      // KEYBOARDHIDDEN_NO
            hard_keyboard_hidden: 1, // HARDKEYBOARDHIDDEN_NO
            navigation: 1,           // NAVIGATION_NONAV
            navigation_hidden: 2,    // NAVIGATIONHIDDEN_YES
            orientation: if w >= h { 2 } else { 1 },
            screen_layout: 0x04 | 0x10 | 0x100, // SCREENLAYOUT_SIZE_XLARGE | LONG_NO | ROUND_NO
            color_mode: 0x1 | 0x4,               // wide color no, hdr no
            ui_mode: 0x1 | 0x10,                 // UI_MODE_TYPE_NORMAL | UI_MODE_NIGHT_NO
            screen_width_dp: wdp,
            screen_height_dp: hdp,
            smallest_screen_width_dp: wdp.min(hdp),
            density_dpi: dpi,
            window: WindowConfiguration::fullscreen(w, h),
            seq: 1,
            font_weight_adjustment: 0,
            grammatical_gender: 0,
        }
    }

    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        p.write_f32(self.font_scale)?;
        p.write_i32(0)?; // mcc
        p.write_i32(0)?; // mnc
        ap::typed(p, |p| ap::string8(p, Some(&self.locales)))?; // LocaleList
        p.write_i32(self.user_set_locale as i32)?;
        p.write_i32(self.touchscreen)?;
        p.write_i32(self.keyboard)?;
        p.write_i32(self.keyboard_hidden)?;
        p.write_i32(self.hard_keyboard_hidden)?;
        p.write_i32(self.navigation)?;
        p.write_i32(self.navigation_hidden)?;
        p.write_i32(self.orientation)?;
        p.write_i32(self.screen_layout)?;
        p.write_i32(self.color_mode)?;
        p.write_i32(self.ui_mode)?;
        p.write_i32(self.screen_width_dp)?;
        p.write_i32(self.screen_height_dp)?;
        p.write_i32(self.smallest_screen_width_dp)?;
        p.write_i32(self.density_dpi)?;
        p.write_i32(0)?; // compatScreenWidthDp
        p.write_i32(0)?; // compatScreenHeightDp
        p.write_i32(0)?; // compatSmallestScreenWidthDp
        self.window.write(p)?;
        p.write_i32(0)?; // assetsSeq
        p.write_i32(self.seq)?;
        p.write_i32(self.font_weight_adjustment)?;
        p.write_i32(self.grammatical_gender)
    }
}

/// android.content.res.CompatibilityInfo (DEFAULT_COMPATIBILITY_INFO shape).
pub struct CompatibilityInfo;

impl CompatibilityInfo {
    pub fn write_default(p: &mut Parcel) -> Result<()> {
        p.write_i32(0)?; // mCompatibilityFlags
        p.write_i32(0)?; // applicationDensity (0 = use device density)
        p.write_f32(1.0)?; // applicationScale
        p.write_f32(1.0)?; // applicationInvertedScale
        p.write_f32(1.0)?; // applicationDensityScale
        p.write_f32(1.0)?; // applicationDensityInvertedScale
        // cameraCompatibilityInfo: the reader dereferences it, so send defaults.
        ap::typed(p, |p| {
            p.write_i32(0)?; // mRotateAndCropRotation (none)
            ap::boolean(p, false)?; // mShouldOverrideSensorOrientation
            ap::boolean(p, false)?; // mShouldLetterboxForCameraCompat
            p.write_i32(0)?; // mDisplayRotationSandbox
            ap::boolean(p, false)?; // mShouldAllowTransformInverseDisplay
            ap::boolean(p, false) // mShouldOverrideLensFacingFrontToBack
        })?;
        ap::null_array(p) // overrideDensityDisplayIds
    }
}

/// android.content.ComponentName: two UTF-16 strings.
pub fn write_component_name(p: &mut Parcel, package: Option<&str>, class: Option<&str>) -> Result<()> {
    match (package, class) {
        (Some(pk), Some(cl)) => {
            ap::string16(p, Some(pk))?;
            ap::string16(p, Some(cl))
        }
        _ => ap::string16(p, None), // ComponentName.writeToParcel(null, out)
    }
}

/// android.content.Intent (minimal explicit launch intent).
#[derive(Clone, Debug, Default)]
pub struct Intent {
    pub action: Option<String>,
    pub package: Option<String>,
    pub component: Option<(String, String)>,
    pub categories: Vec<String>,
    pub flags: i32,
}

pub const FLAG_ACTIVITY_NEW_TASK: i32 = 0x1000_0000;

impl Intent {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        ap::string8(p, self.action.as_deref())?; // mAction
        p.write_i32(0)?; // Uri.writeToParcel(null): NULL_TYPE_ID
        ap::string8(p, None)?; // mType
        ap::string8(p, None)?; // mIdentifier
        p.write_i32(self.flags)?; // mFlags
        p.write_i32(0)?; // mExtendedFlags
        ap::string8(p, self.package.as_deref())?; // mPackage
        write_component_name(p, self.component.as_ref().map(|c| c.0.as_str()), self.component.as_ref().map(|c| c.1.as_str()))?;
        p.write_i32(0)?; // mSourceBounds: absent
        if self.categories.is_empty() {
            p.write_i32(0)?;
        } else {
            p.write_i32(self.categories.len() as i32)?;
            for c in &self.categories {
                ap::string8(p, Some(c))?;
            }
        }
        p.write_i32(0)?; // mSelector: absent
        p.write_i32(0)?; // mClipData: absent
        p.write_i32(-2)?; // mContentUserHint: UserHandle.USER_CURRENT
        ap::null_array(p)?; // mExtras: writeBundle(null)
        p.write_i32(0)?; // mOriginalIntent: absent
        p.write_i32(0) // creator token info: absent (Flags.preventIntentRedirect() is true in this image)
    }
}

/// android.content.pm.ActivityInfo (with its ComponentInfo/PackageItemInfo prefix in
/// ComponentInfo's compact form: name, presence mask, then only the fields the mask
/// says are present; we set no "same as ApplicationInfo" bits so every field is explicit).
#[derive(Clone, Debug)]
pub struct ActivityInfo {
    pub name: String,
    pub package_name: String,
    pub application_info: ApplicationInfo,
    pub process_name: String,
    pub theme: i32,
    pub launch_mode: i32,
    pub flags: i32,
    pub config_changes: i32,
    pub screen_orientation: i32,
    pub soft_input_mode: i32,
    pub task_affinity: Option<String>,
}

impl ActivityInfo {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        // ComponentInfo
        ap::string8(p, Some(&self.name))?;
        p.write_i32(1 | 2)?; // enabled | exported; no fields omitted
        ap::string8(p, Some(&self.package_name))?; // packageName
        p.write_i32(0)?; // labelRes
        ap::char_sequence(p, None)?; // nonLocalizedLabel
        p.write_i32(0)?; // icon
        p.write_i32(0)?; // logo
        ap::null_array(p)?; // metaData
        p.write_i32(0)?; // banner
        p.write_i32(0)?; // showUserIcon
        self.application_info.write(p)?;
        ap::string8(p, Some(&self.process_name))?; // processName
        ap::string8(p, None)?; // splitName
        ap::null_array(p)?; // attributionTags
        p.write_i32(0)?; // descriptionRes
        // ActivityInfo
        p.write_i32(self.theme)?;
        p.write_i32(self.launch_mode)?;
        p.write_i32(0)?; // documentLaunchMode
        ap::string8(p, None)?; // permission
        ap::string8(p, self.task_affinity.as_deref())?;
        ap::string8(p, None)?; // targetActivity
        ap::string8(p, None)?; // launchToken
        p.write_i32(self.flags)?;
        p.write_i32(0)?; // privateFlags
        p.write_i32(self.screen_orientation)?;
        p.write_i32(self.config_changes)?;
        p.write_i32(self.soft_input_mode)?;
        p.write_i32(0)?; // uiOptions
        ap::string8(p, None)?; // parentActivityName
        p.write_i32(0)?; // persistableMode
        p.write_i32(0)?; // maxRecents
        p.write_i32(0)?; // lockTaskLaunchMode
        p.write_i32(0)?; // windowLayout: absent
        p.write_i32(2)?; // resizeMode: RESIZE_MODE_RESIZEABLE
        ap::string8(p, None)?; // requestedVrComponent
        p.write_i32(-1)?; // rotationAnimation: ROTATION_ANIMATION_UNSPECIFIED
        p.write_i32(0)?; // colorMode
        p.write_f32(0.0)?; // mMaxAspectRatio
        p.write_f32(0.0)?; // mMinAspectRatio
        ap::boolean(p, true)?; // supportsSizeChanges
        ap::null_array(p)?; // knownActivityEmbeddingCerts: ForStringSet(null)
        ap::string8(p, None)?; // requiredDisplayCategory
        p.write_i32(0) // requireContentUriPermissionFromCaller
    }
}

/// android.window.ActivityWindowInfo
pub fn write_activity_window_info(p: &mut Parcel, task: Rect) -> Result<()> {
    ap::boolean(p, false)?; // mIsEmbedded
    task.write(p)?; // mTaskBounds
    task.write(p) // mTaskFragmentBounds
}

/// A `ClientTransaction` carrying LaunchActivityItem + ResumeActivityItem, as
/// `IApplicationThread.scheduleTransaction` expects it (`writeTypedObject`).
pub struct LaunchTransaction<'a> {
    pub activity_token: &'a rsbinder::SIBinder,
    pub assist_token: &'a rsbinder::SIBinder,
    pub shareable_token: &'a rsbinder::SIBinder,
    pub client_controller: &'a rsbinder::SIBinder,
    pub ident: i32,
    pub config: &'a Configuration,
    pub intent: &'a Intent,
    pub info: &'a ActivityInfo,
    pub display_id: i32,
    pub task_bounds: Rect,
}

pub const PROCESS_STATE_TOP: i32 = 2;

impl LaunchTransaction<'_> {
    pub fn write(&self, p: &mut Parcel) -> Result<()> {
        p.write_i32(1)?; // typed object marker
        p.write_i32(2)?; // writeParcelableList: two items
        // --- LaunchActivityItem
        ap::string16(p, Some("android.app.servertransaction.LaunchActivityItem"))?;
        p.write(&Some(self.activity_token.clone()))?; // mActivityToken
        p.write_i32(self.ident)?; // mIdent
        ap::typed(p, |p| self.config.write(p))?; // mCurConfig
        ap::typed(p, |p| self.config.write(p))?; // mOverrideConfig
        p.write_i32(0)?; // mDeviceId
        ap::string16(p, None)?; // mReferrer
        p.write(&None::<rsbinder::SIBinder>)?; // mVoiceInteractor
        p.write_i32(PROCESS_STATE_TOP)?; // mProcState
        ap::null_array(p)?; // mState: writeBundle(null)
        ap::null_array(p)?; // mPersistentState: writePersistableBundle(null)
        ap::null_array(p)?; // mPendingResults
        ap::null_array(p)?; // mPendingNewIntents
        ap::typed_none(p)?; // mSceneTransitionInfo
        ap::boolean(p, false)?; // mIsForward
        ap::typed_none(p)?; // mProfilerInfo
        p.write(&Some(self.assist_token.clone()))?; // mAssistToken
        p.write(&Some(self.client_controller.clone()))?; // mActivityClientController
        p.write(&Some(self.shareable_token.clone()))?; // mShareableActivityToken
        ap::boolean(p, false)?; // mLaunchedFromBubble
        p.write(&None::<rsbinder::SIBinder>)?; // mTaskFragmentToken
        p.write(&None::<rsbinder::SIBinder>)?; // mInitialCallerInfoAccessToken
        ap::typed(p, |p| write_activity_window_info(p, self.task_bounds))?; // mActivityWindowInfo
        p.write_i32(self.display_id)?; // mDisplayId
        ap::typed(p, |p| self.intent.write(p))?; // mIntent
        ap::typed(p, |p| self.info.write(p))?; // mInfo
        // --- ResumeActivityItem
        ap::string16(p, Some("android.app.servertransaction.ResumeActivityItem"))?;
        p.write(&Some(self.activity_token.clone()))?; // ActivityTransactionItem.mActivityToken
        p.write_i32(PROCESS_STATE_TOP)?; // mProcState
        ap::boolean(p, false)?; // mIsForward
        ap::boolean(p, false) // mShouldSendCompatFakeFocus
    }
}
