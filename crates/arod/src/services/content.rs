//! `content`: android.content.IContentService.
//!
//! Jetpack Compose (and View-based animation runners) monitor system settings
//! such as `animator_duration_scale` by registering a ContentObserver with
//! ContentResolver (`ContentResolver.registerContentObserver`). If the "content"
//! service is missing, ContentResolver throws a NullPointerException in the
//! recomposer coroutine, cancelling its coroutine scope and killing the frame loop.
//!
//! We publish a minimal IContentService that accepts observer registrations and
//! sync requests cleanly.

use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct ContentService;

impl Service for ContentService {
    const DESCRIPTOR: &'static str = "android.content.IContentService";
    const TABLE: &'static [(u32, &'static str)] = codes::ICONTENTSERVICE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "registerContentObserver"
            | "unregisterContentObserver"
            | "notifyChange"
            | "requestSync"
            | "sync"
            | "syncAsUser"
            | "cancelSync"
            | "cancelSyncAsUser"
            | "cancelRequest"
            | "setSyncAutomatically"
            | "setSyncAutomaticallyAsUser"
            | "addPeriodicSync"
            | "removePeriodicSync"
            | "setIsSyncable"
            | "setIsSyncableAsUser"
            | "setMasterSyncAutomatically"
            | "setMasterSyncAutomaticallyAsUser"
            | "addStatusChangeListener"
            | "removeStatusChangeListener"
            | "putCache"
            | "resetTodayStats"
            | "onDbCorruption" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            "getIsSyncable" | "getIsSyncableAsUser" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getMasterSyncAutomatically"
            | "getMasterSyncAutomaticallyAsUser"
            | "getSyncAutomatically"
            | "getSyncAutomaticallyAsUser"
            | "isSyncActive"
            | "isSyncPending"
            | "isSyncPendingAsUser" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getCurrentSyncs"
            | "getCurrentSyncsAsUser"
            | "getPeriodicSyncs"
            | "getSyncAdapterTypes"
            | "getSyncAdapterTypesAsUser" => {
                ap::no_exception(reply)?;
                ap::empty_list(reply)?;
                Ok(true)
            }
            "getSyncAdapterPackagesForAuthorityAsUser" => {
                ap::no_exception(reply)?;
                ap::null_array(reply)?;
                Ok(true)
            }
            "getSyncAdapterPackageAsUser" => {
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
                Ok(true)
            }
            "getSyncStatus" | "getSyncStatusAsUser" | "getCache" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
        }
    }
}
