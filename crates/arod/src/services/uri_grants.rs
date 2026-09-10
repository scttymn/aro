//! `uri_grants`: android.app.IUriGrantsManager.
//! Handles persistable URI permission grants requested by apps.

use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct UriGrantsService;

const TABLE: &'static [(u32, &'static str)] = &[
    (1, "takePersistableUriPermission"),
    (2, "releasePersistableUriPermission"),
    (3, "grantUriPermissionFromOwner"),
    (4, "getGrantedUriPermissions"),
    (5, "clearGrantedUriPermissions"),
    (6, "getUriPermissions"),
    (7, "checkGrantUriPermission_ignoreNonSystem"),
];

impl Service for UriGrantsService {
    const DESCRIPTOR: &'static str = "android.app.IUriGrantsManager";
    const TABLE: &'static [(u32, &'static str)] = TABLE;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "takePersistableUriPermission" | "releasePersistableUriPermission" | "clearGrantedUriPermissions" => {
                log::info!("uri_grants: {name}");
                ap::no_exception(reply)?;
                Ok(true)
            }
            "getGrantedUriPermissions" | "getUriPermissions" => {
                // ParceledListSlice, empty: [hasParcelableList = 1][size = 0]
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => {
                log::info!("uri_grants: {name} (default no-op)");
                ap::no_exception(reply)?;
                Ok(true)
            }
        }
    }
}
