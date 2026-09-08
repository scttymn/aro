//! `activity_task`: android.app.IActivityTaskManager, and the
//! IActivityClientController the app uses for per-activity calls.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::Mutex;

pub struct ActivityTaskService {
    pub client_controller: Mutex<Option<SIBinder>>,
}

impl Service for ActivityTaskService {
    const DESCRIPTOR: &'static str = "android.app.IActivityTaskManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYTASKMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getActivityClientController" => {
                ap::no_exception(reply)?;
                reply.write(&self.client_controller.lock().unwrap().clone())?;
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
