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
            "startActivity" | "startActivityWithFeature" | "startActivityAsUser" => {
                // (IApplicationThread caller, String callingPackage, String callingFeatureId,
                //  Intent intent, String resolvedType, IBinder resultTo, String resultWho, int requestCode, int flags, ...)
                let _caller: Option<SIBinder> = data.read()?;
                let _calling_pkg: Option<String> = data.read()?; // String16
                let _feature: Option<String> = data.read()?;     // String16
                if data.read_i32()? != 0 {
                    let target = crate::pending_intent::read_intent_target(data)?;
                    let _resolved_type: Option<String> = data.read().ok().flatten();
                    let result_to: Option<SIBinder> = data.read().ok().flatten();
                    let result_who: Option<String> = data.read().ok().flatten();
                    let request_code: i32 = data.read_i32().unwrap_or(0);
                    let _flags: i32 = data.read_i32().unwrap_or(0);

                    log::info!("activity_task: startActivity action={:?} result_to={:?} req_code={request_code}", target.action, result_to.is_some());

                    if target.action.as_deref() == Some("android.intent.action.OPEN_DOCUMENT")
                        || target.action.as_deref() == Some("android.intent.action.GET_CONTENT")
                    {
                        self.activity.open_document(result_to, result_who, request_code);
                    } else {
                        self.activity.start_activity_target(target);
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
pub struct ActivityClientController {
    pub activity: std::sync::Arc<super::activity::ActivityService>,
}

impl Service for ActivityClientController {
    const DESCRIPTOR: &'static str = "android.app.IActivityClientController";
    const TABLE: &'static [(u32, &'static str)] = codes::IACTIVITYCLIENTCONTROLLER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "activityIdle" | "activityResumed" | "activityPaused" | "activityStopped" | "activityDestroyed" | "activityTopResumedStateLost" | "activityRefreshed" | "activityLocalRelaunch" | "activityRelaunched" => {
                let _token: Option<SIBinder> = data.read()?;
                log::info!("activity-client: {name}");
                if name == "activityDestroyed" {
                    let mut stack = self.activity.activity_stack.lock().unwrap();
                    let popped = stack.pop();
                    log::info!("activity-client: activityDestroyed popped {popped:?}, remaining stack: {stack:?}");
                }
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
            "finishActivity" => {
                let token: Option<SIBinder> = data.read().ok().flatten();
                let result_code = data.read_i32().unwrap_or(0);
                let has_intent = data.read_i32().unwrap_or(0);
                if has_intent != 0 {
                    let _action = ap::read_string8(data).ok();
                    let uri_type = data.read_i32().unwrap_or(0);
                    if uri_type != 0 {
                        let _ = ap::read_string8(data).ok();
                    }
                }
                let finish_task = data.read_i32().unwrap_or(0);
                log::info!("activity-client: finishActivity token={token:?} resultCode={result_code} finishTask={finish_task}");
                if let Some(token) = token {
                    self.activity.destroy_activity(token);
                }
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "finishActivityAffinity" => {
                let token: Option<SIBinder> = data.read().ok().flatten();
                log::info!("activity-client: finishActivityAffinity token={token:?}");
                if let Some(token) = token {
                    self.activity.destroy_activity(token);
                }
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
