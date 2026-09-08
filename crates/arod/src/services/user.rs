//! `user`: android.os.IUserManager. One user, id 0, unlocked, no restrictions.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct UserService;

const USER_ID: i32 = 0;
const FLAG_PRIMARY: i32 = 0x0000_0001;
const FLAG_ADMIN: i32 = 0x0000_0002;
const FLAG_INITIALIZED: i32 = 0x0000_0010;
const FLAG_FULL: i32 = 0x0000_0400;
const FLAG_SYSTEM: i32 = 0x0000_0800;
const FLAG_MAIN: i32 = 0x0000_4000;

/// android.content.pm.UserInfo for the one user.
fn write_user_info(p: &mut Parcel) -> Result<()> {
    p.write_i32(USER_ID)?; // id
    ap::string8(p, Some("Owner"))?; // name
    ap::string8(p, None)?; // iconPath
    p.write_i32(FLAG_PRIMARY | FLAG_ADMIN | FLAG_INITIALIZED | FLAG_FULL | FLAG_SYSTEM | FLAG_MAIN)?; // flags
    ap::string8(p, Some("android.os.usertype.full.SYSTEM"))?; // userType
    p.write_i32(0)?; // serialNumber
    p.write_i64(0)?; // creationTime
    p.write_i64(0)?; // lastLoggedInTime
    ap::string8(p, None)?; // lastLoggedInFingerprint
    ap::boolean(p, false)?; // partial
    ap::boolean(p, false)?; // preCreated
    p.write_i32(-10000)?; // profileGroupId: NO_PROFILE_GROUP_ID
    ap::boolean(p, false)?; // guestToRemove
    p.write_i32(-10000)?; // restrictedProfileParentId: NO_PROFILE_GROUP_ID
    p.write_i32(0) // profileBadge
}

impl Service for UserService {
    const DESCRIPTOR: &'static str = "android.os.IUserManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IUSERMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "isUserUnlocked" | "isUserUnlockingOrUnlocked" | "isUserRunning" | "isUserForeground" | "isUserAdmin" => {
                let _user = data.read_i32()?;
                ap::no_exception(reply)?;
                ap::boolean(reply, true)?;
                Ok(true)
            }
            "isManagedProfile" | "isQuietModeEnabled" | "isSameProfileGroup" | "isUserOfType" | "isDemoUser" | "isRestricted" | "isUserSwitcherEnabled" | "isUserNameSet" | "isProfile" | "isCloneProfile" | "isPrivateProfile" | "isUserTypeEnabled" | "isHeadlessSystemUserMode" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "hasUserRestriction" | "hasUserRestrictionOnAnyUser" | "hasBaseUserRestriction" => {
                let _restriction: Option<String> = data.read()?;
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getUserSerialNumber" | "getUserHandle" | "getMainDisplayIdAssignedToUser" => {
                let _arg = data.read_i32()?;
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getProfileParentId" => {
                let user = data.read_i32()?;
                ap::no_exception(reply)?;
                reply.write_i32(user)?;
                Ok(true)
            }
            "getUserInfo" => {
                let user = data.read_i32()?;
                ap::no_exception(reply)?;
                if user == USER_ID {
                    ap::typed(reply, write_user_info)?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getProfileIds" => {
                ap::no_exception(reply)?;
                ap::int_array(reply, Some(&[USER_ID]))?;
                Ok(true)
            }
            "getUsers" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                ap::typed(reply, write_user_info)?;
                Ok(true)
            }
            "getUserName" => {
                ap::no_exception(reply)?;
                ap::string16(reply, Some("Owner"))?;
                Ok(true)
            }
            "getUserStartRealtime" | "getUserUnlockRealtime" => {
                ap::no_exception(reply)?;
                reply.write_i64(0)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
