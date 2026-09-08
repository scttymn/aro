//! `connectivity`: android.net.IConnectivityManager. Reports the one network
//! the host has (mirrored from NetworkManager via crate::hostnet). Apps share
//! the host network namespace, so this describes their real connection.
use super::Service;
use crate::aparcel as ap;
use crate::hostnet::HostNet;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct NetworkService {
    pub net: HostNet,
}

const NET_ID: i32 = 100;

// NET_CAPABILITY_* bit positions.
const CAP_NOT_METERED: i64 = 11;
const CAP_INTERNET: i64 = 12;
const CAP_NOT_RESTRICTED: i64 = 13;
const CAP_TRUSTED: i64 = 14;
const CAP_NOT_VPN: i64 = 15;
const CAP_VALIDATED: i64 = 16;
const CAP_NOT_ROAMING: i64 = 18;
const CAP_NOT_CONGESTED: i64 = 20;
const CAP_NOT_SUSPENDED: i64 = 21;
const CAP_NOT_VCN_MANAGED: i64 = 28;

impl NetworkService {
    fn caps_mask(&self) -> i64 {
        let mut m = 0i64;
        for b in [CAP_INTERNET, CAP_NOT_RESTRICTED, CAP_TRUSTED, CAP_NOT_VPN, CAP_NOT_ROAMING, CAP_NOT_CONGESTED, CAP_NOT_SUSPENDED, CAP_NOT_VCN_MANAGED] {
            m |= 1 << b;
        }
        if self.net.validated {
            m |= 1 << CAP_VALIDATED;
        }
        if !self.net.metered {
            m |= 1 << CAP_NOT_METERED;
        }
        m
    }

    /// android.net.Network: netId, then (Android B+) two booleans.
    fn write_network(p: &mut Parcel) -> Result<()> {
        p.write_i32(NET_ID)?;
        ap::boolean(p, false)?; // mPrivateDnsBypass
        ap::boolean(p, false)?; // (reserved flag, B+)
        Ok(())
    }

    /// android.net.NetworkCapabilities in field order (see spec/notes).
    fn write_capabilities(&self, p: &mut Parcel) -> Result<()> {
        p.write_i64(self.caps_mask())?; // mNetworkCapabilities
        p.write_i64(0)?; // mForbiddenNetworkCapabilities
        p.write_i64(1 << self.net.transport as i64)?; // mTransportTypes
        p.write_i32(0)?; // mLinkUpBandwidthKbps
        p.write_i32(0)?; // mLinkDownBandwidthKbps
        p.write_i32(-1)?; // mNetworkSpecifier: writeParcelable(null)
        p.write_i32(-1)?; // mTransportInfo: writeParcelable(null)
        p.write_i32(i32::MIN)?; // mSignalStrength: UNSPECIFIED
        p.write_i32(-1)?; // mUids: writeParcelableArraySet(null)
        p.write_i32(0)?; // mAllowedUids: empty int array
        ap::string16(p, None)?; // mSSID
        ap::boolean(p, false)?; // mPrivateDnsBroken
        p.write_i32(0)?; // administratorUids: empty int array
        p.write_i32(-1)?; // mOwnerUid: INVALID_UID
        p.write_i32(-1)?; // mRequestorUid
        ap::string16(p, None)?; // mRequestorPackageName
        p.write_i32(0)?; // mSubscriptionIds: empty int array
        p.write_i32(-1)?; // mUnderlyingNetworks: writeTypedList(null)
        p.write_i32(0)?; // mEnterpriseId
        p.write_i32(0)?; // mReservationId
        ap::boolean(p, false)?; // mMatchNonThreadLocalNetworks
        Ok(())
    }

    /// android.net.NetworkInfo (legacy).
    fn write_network_info(&self, p: &mut Parcel) -> Result<()> {
        let (ty, name) = match self.net.transport {
            crate::hostnet::TRANSPORT_WIFI => (1, "WIFI"),
            crate::hostnet::TRANSPORT_CELLULAR => (0, "MOBILE"),
            _ => (9, "ETHERNET"),
        };
        let state = if self.net.online { "CONNECTED" } else { "DISCONNECTED" };
        p.write_i32(ty)?; // mNetworkType
        p.write_i32(0)?; // mSubtype
        ap::string16(p, Some(name))?; // mTypeName
        ap::string16(p, Some(""))?; // mSubtypeName
        ap::string16(p, Some(state))?; // State.name()
        ap::string16(p, Some(state))?; // DetailedState.name()
        p.write_i32(0)?; // mIsFailover
        p.write_i32(self.net.online as i32)?; // mIsAvailable
        p.write_i32(0)?; // mIsRoaming
        ap::string16(p, None)?; // mReason
        ap::string16(p, None)?; // mExtraInfo
        Ok(())
    }
}

impl Service for NetworkService {
    const DESCRIPTOR: &'static str = "android.net.IConnectivityManager";
    const TABLE: &'static [(u32, &'static str)] = super::network_codes::ICONNECTIVITYMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getActiveNetwork" | "getActiveNetworkForUid" => {
                ap::no_exception(reply)?;
                if self.net.online {
                    ap::typed(reply, |p| Self::write_network(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getNetworkCapabilities" => {
                ap::no_exception(reply)?;
                if self.net.online {
                    ap::typed(reply, |p| self.write_capabilities(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getActiveNetworkInfo" | "getActiveNetworkInfoForUid" | "getNetworkInfo" | "getNetworkInfoForUid" => {
                ap::no_exception(reply)?;
                if self.net.online {
                    ap::typed(reply, |p| self.write_network_info(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getAllNetworks" => {
                // Network[]
                ap::no_exception(reply)?;
                if self.net.online {
                    reply.write_i32(1)?;
                    Self::write_network(reply)?;
                } else {
                    reply.write_i32(0)?;
                }
                Ok(true)
            }
            "getLinkProperties" | "getActiveLinkProperties" => {
                // LinkProperties are optional for basic use; report none for now.
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "isActiveNetworkMetered" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, self.net.metered)?;
                Ok(true)
            }
            "isDefaultNetworkActive" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, self.net.online)?;
                Ok(true)
            }
            _ => {
                ap::no_exception(reply)?;
                if name.starts_with("is") || name.starts_with("are") || name.starts_with("request") && name != "requestNetwork" {
                    ap::boolean(reply, false)?;
                } else if name.starts_with("get") {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
        }
    }
}
