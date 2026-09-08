//! `package`: android.content.pm.IPackageManager.
use super::registry::Registry;
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};
use std::sync::Arc;

pub struct PackageService {
    pub registry: Arc<Registry>,
}

const PERMISSION_GRANTED: i32 = 0;
const COMPONENT_ENABLED_STATE_DEFAULT: i32 = 0;

impl Service for PackageService {
    const DESCRIPTOR: &'static str = "android.content.pm.IPackageManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IPACKAGEMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getApplicationInfo" => {
                let pkg: Option<String> = data.read()?;
                let _flags = data.read_i64()?;
                let _user = data.read_i32()?;
                ap::no_exception(reply)?;
                match pkg.as_deref().and_then(|p| self.registry.find(p)) {
                    Some(spec) => ap::typed(reply, |p| Registry::application_info(&spec).write(p))?,
                    None => ap::typed_none(reply)?,
                }
                Ok(true)
            }
            "getPackageInfo" => {
                let pkg: Option<String> = data.read()?;
                let _flags = data.read_i64()?;
                let _user = data.read_i32()?;
                ap::no_exception(reply)?;
                match pkg.as_deref().and_then(|p| self.registry.find(p)) {
                    Some(spec) => ap::typed(reply, |p| Registry::write_package_info(&spec, p))?,
                    None => ap::typed_none(reply)?,
                }
                Ok(true)
            }
            "getPackagesForUid" => {
                let uid = data.read_i32()?;
                ap::no_exception(reply)?;
                match self.registry.find_uid(uid) {
                    Some(spec) => {
                        reply.write_i32(1)?;
                        ap::string16(reply, Some(&spec.package))?;
                    }
                    None => ap::null_array(reply)?,
                }
                Ok(true)
            }
            "getNameForUid" => {
                let uid = data.read_i32()?;
                ap::no_exception(reply)?;
                ap::string16(reply, self.registry.find_uid(uid).map(|s| s.package).as_deref())?;
                Ok(true)
            }
            "getPackageUid" => {
                let pkg: Option<String> = data.read()?;
                ap::no_exception(reply)?;
                reply.write_i32(pkg.as_deref().and_then(|p| self.registry.find(p)).map(|s| s.uid).unwrap_or(-1))?;
                Ok(true)
            }
            "isPackageAvailable" => {
                let pkg: Option<String> = data.read()?;
                ap::no_exception(reply)?;
                ap::boolean(reply, pkg.as_deref().and_then(|p| self.registry.find(p)).is_some())?;
                Ok(true)
            }
            "hasSystemFeature" => {
                let feature: Option<String> = data.read()?;
                let _version = data.read_i32()?;
                log::info!("package: hasSystemFeature {feature:?} -> false");
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "checkPermission" => {
                let perm: Option<String> = data.read()?;
                let pkg: Option<String> = data.read()?;
                log::info!("package: checkPermission {perm:?} for {pkg:?} -> granted");
                ap::no_exception(reply)?;
                reply.write_i32(PERMISSION_GRANTED)?;
                Ok(true)
            }
            "getInstallerPackageName" => {
                let _pkg: Option<String> = data.read()?;
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
                Ok(true)
            }
            "getComponentEnabledSetting" => {
                ap::no_exception(reply)?;
                reply.write_i32(COMPONENT_ENABLED_STATE_DEFAULT)?;
                Ok(true)
            }
            "getActivityInfo" => {
                // ComponentName typed: marker, then two UTF-16 strings
                let present = data.read_i32()?;
                let (pkg, cls): (Option<String>, Option<String>) = if present != 0 { (data.read()?, data.read()?) } else { (None, None) };
                let _flags = data.read_i64()?;
                let _user = data.read_i32()?;
                ap::no_exception(reply)?;
                let info = pkg.as_deref().and_then(|p| self.registry.find(p)).and_then(|spec| cls.as_deref().and_then(|c| Registry::activity_info(&spec, c)));
                match info {
                    Some(ai) => ap::typed(reply, |p| ai.write(p))?,
                    None => {
                        log::warn!("package: getActivityInfo {pkg:?}/{cls:?}: unknown");
                        ap::typed_none(reply)?
                    }
                }
                Ok(true)
            }
            "getServiceInfo" | "getReceiverInfo" | "getProviderInfo" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "queryProperty" | "queryProperties" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "getSdkSandboxPackageName" => {
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
                Ok(true)
            }
            "getPropertyAsUser" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "notifyDexLoad" => Ok(true), // oneway; nothing to record yet
            _ => Ok(false),
        }
    }
}
