//! `activity_task`: android.app.IActivityTaskManager, and the
//! IActivityClientController the app uses for per-activity calls.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::Mutex;

pub struct ActivityTaskService {
    pub client_controller: Mutex<Option<SIBinder>>,
    pub activity: std::sync::Arc<super::activity::ActivityService>,
}

impl Service for ActivityTaskService {
    const DESCRIPTOR: &'static str = "android.app.IActivityTaskManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYTASKMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getActivityClientController" => {
                ap::no_exception(reply)?;
                reply.write(&self.client_controller.lock().unwrap().clone())?;
                Ok(true)
            }
            "startActivity" | "startActivityWithFeature" => {
                // (IApplicationThread caller, String callingPackage, String callingFeatureId,
                //  Intent intent, String resolvedType, IBinder resultTo, ...)
                let _caller: Option<SIBinder> = data.read()?;
                let _calling_pkg: Option<String> = data.read()?; // String16
                let _feature: Option<String> = data.read()?;     // String16
                if data.read_i32()? != 0 {
                    // Intent body (see android.content.Intent.writeToParcel).
                    let action = ap::read_string8(data)?;
                    let uri_type = data.read_i32()?; // Uri.writeToParcel: 0 null, 1 StringUri
                    let data_uri = match uri_type {
                        0 => None,
                        1 => ap::read_string8(data)?, // StringUri: uriString
                        other => {
                            log::warn!("activity_task: startActivity data Uri type {other} unsupported; treating as no data");
                            None
                        }
                    };
                    // Only the StringUri (or null) shapes keep the parcel aligned for the fields
                    // below; for other shapes we still resolve on action alone.
                    if uri_type == 0 || uri_type == 1 {
                        let _type = ap::read_string8(data)?;
                        let _ident = ap::read_string8(data)?;
                        let _flags = data.read_i32()?;
                        let _ext_flags = data.read_i32()?;
                        let intent_pkg = ap::read_string8(data)?;
                        let comp_pkg: Option<String> = data.read()?; // ComponentName: String16 package
                        let comp_cls: Option<String> = if comp_pkg.is_some() { data.read()? } else { None };
                        self.activity.start_activity(comp_pkg.or(intent_pkg), comp_cls, action, data_uri);
                    } else {
                        self.activity.start_activity(None, None, action, None);
                    }
                }
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // START_SUCCESS
                Ok(true)
            }
            "supportsMultiWindow" | "supportsSplitScreenMultiWindow" | "supportsLocalVoiceInteraction" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, name == "supportsMultiWindow")?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// android.app.IActivityClientController: the app reports lifecycle here.
pub struct ActivityClientController;

impl Service for ActivityClientController {
    const DESCRIPTOR: &'static str = "android.app.IActivityClientController";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYCLIENTCONTROLLER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "activityIdle" | "activityResumed" | "activityPaused" | "activityStopped" | "activityDestroyed" | "activityTopResumedStateLost" | "activityRefreshed" | "activityLocalRelaunch" | "activityRelaunched" => {
                let _token: Option<SIBinder> = data.read()?;
                log::info!("activity-client: {name}");
                ap::no_exception(reply).ok();
                Ok(true)
            }
            "getRequestedOrientation" => {
                let _token: Option<SIBinder> = data.read()?;
                ap::no_exception(reply)?;
                reply.write_i32(-1)?; // SCREEN_ORIENTATION_UNSPECIFIED
                Ok(true)
            }
            "setRequestedOrientation" | "setTaskDescription" | "reportSizeConfigurations" | "setImmersive" | "setRecentsScreenshotEnabled" | "overrideActivityTransition" | "clearOverrideActivityTransition" | "setShouldDockBigOverlays" | "setPictureInPictureParams" | "setForceSendResultForMediaProjection" | "onPictureInPictureUiStateChanged" | "reportActivityFullyDrawn" => {
                ap::no_exception(reply).ok();
                Ok(true)
            }
            "isImmersive" | "isTopOfTask" | "willActivityBeVisible" | "isRootVoiceInteraction" | "shouldUpRecreateTask" | "navigateUpTo" | "moveActivityTaskToBack" | "convertFromTranslucent" | "convertToTranslucent" | "enterPictureInPictureMode" | "requestVisibleBehind" | "showAssistFromActivity" | "isRequestedToLaunchInTaskFragment" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getTaskForActivity" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                Ok(true)
            }
            "getDisplayId" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getActivityCallerToken" | "getActivityCallerPackage" | "getLaunchedFromPackage" | "getCallingPackage" => {
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
