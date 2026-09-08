//! `display`: android.hardware.display.IDisplayManager.
//!
//! ARO presents one logical display per session for now: the "default
//! display" (id 0) sized like the desktop window the app will get. Parcel
//! layouts follow `spec/parcels/android.view.DisplayInfo.txt` and friends.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, TransactionCode};
use std::sync::Mutex;

pub struct DisplayService {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub dpi: i32,
    pub callbacks: Mutex<Vec<(SIBinder, i64)>>,
}

const DISPLAY_ID_DEFAULT: i32 = 0;
const TYPE_INTERNAL: i32 = 1;
const STATE_ON: i32 = 2;
const FLAG_SUPPORTS_PROTECTED_BUFFERS: i32 = 1 << 0;
const FLAG_SECURE: i32 = 1 << 1;
const FLAG_TRUSTED: i32 = 1 << 7;

impl DisplayService {
    /// android.view.Display.Mode
    fn write_mode(&self, p: &mut Parcel) -> Result<()> {
        p.write_i32(1)?; // mModeId
        p.write_i32(1)?; // mParentModeId
        p.write_i32(1)?; // mSfModeId
        p.write_i32(0)?; // mFlags
        p.write_i32(self.width)?;
        p.write_i32(self.height)?;
        p.write_f32(60.0)?; // mPeakRefreshRate
        p.write_f32(60.0)?; // mVsyncRate
        p.write_i32(0)?; // mAlternativeRefreshRates: float[0]
        p.write_i32(0) // mSupportedHdrTypes: int[0]
    }

    /// android.view.DisplayInfo (writeToParcel order).
    fn write_display_info(&self, p: &mut Parcel) -> Result<()> {
        let (w, h) = (self.width, self.height);
        p.write_i32(0)?; // layerStack
        p.write_i32(FLAG_SUPPORTS_PROTECTED_BUFFERS | FLAG_SECURE | FLAG_TRUSTED)?; // flags
        p.write_i32(TYPE_INTERNAL)?; // type
        p.write_i32(DISPLAY_ID_DEFAULT)?; // displayId
        p.write_i32(0)?; // displayGroupId
        ap::string16(p, None)?; // address: writeParcelable(null)
        ap::string16(p, None)?; // deviceProductInfo: writeParcelable(null)
        ap::string8(p, Some(&self.name))?; // name (the host output)
        p.write_i32(w)?; // appWidth
        p.write_i32(h)?; // appHeight
        p.write_i32(w.min(h))?; // smallestNominalAppWidth
        p.write_i32(w.min(h))?; // smallestNominalAppHeight
        p.write_i32(w.max(h))?; // largestNominalAppWidth
        p.write_i32(w.max(h))?; // largestNominalAppHeight
        p.write_i32(w)?; // logicalWidth
        p.write_i32(h)?; // logicalHeight
        p.write_i32(0)?; // DisplayCutout: NO_CUTOUT
        p.write_i32(0)?; // rotation
        p.write_i32(1)?; // modeId
        p.write_f32(60.0)?; // renderFrameRate
        ap::boolean(p, false)?; // hasArrSupport
        ap::string16(p, None)?; // frameRateCategoryRate: writeParcelable(null)
        p.write_i32(1)?; // supportedRefreshRates.length
        p.write_f32(60.0)?;
        ap::null_array(p)?; // frameRateVelocityMapping typed list: null
        p.write_i32(1)?; // defaultModeId
        p.write_i32(0)?; // userPreferredModeId
        p.write_i32(1)?; // supportedModes.length
        self.write_mode(p)?;
        p.write_i32(0)?; // colorMode (COLOR_MODE_DEFAULT)
        p.write_i32(1)?; // supportedColorModes.length
        p.write_i32(0)?;
        ap::string16(p, None)?; // hdrCapabilities: writeParcelable(null)
        ap::boolean(p, false)?; // isForceSdr
        ap::boolean(p, false)?; // minimalPostProcessingSupported
        p.write_i32(self.dpi)?; // logicalDensityDpi
        p.write_f32(self.dpi as f32)?; // physicalXDpi
        p.write_f32(self.dpi as f32)?; // physicalYDpi
        p.write_i64(1_000_000)?; // appVsyncOffsetNanos
        p.write_i64(16_666_666)?; // presentationDeadlineNanos
        p.write_i32(STATE_ON)?; // state
        p.write_i32(STATE_ON)?; // committedState
        p.write_i32(-1)?; // ownerUid
        ap::string8(p, None)?; // ownerPackageName
        ap::string8(p, Some("aro:0"))?; // uniqueId
        p.write_i32(0)?; // removeMode
        p.write_f32(0.0)?; // refreshRateOverride
        p.write_f32(0.0)?; // brightnessMinimum
        p.write_f32(1.0)?; // brightnessMaximum
        p.write_f32(0.5)?; // brightnessDefault
        p.write_f32(0.0)?; // brightnessDim
        ap::typed_none(p)?; // roundedCorners
        p.write_i32(0)?; // userDisabledHdrTypes.length
        p.write_i32(0)?; // installOrientation
        // displayShape: rectangular
        ap::typed(p, |p| {
            p.write_i32(0)?; // mType: RECT
            ap::string8(p, Some("aro:0"))?; // mDisplayUniqueId
            p.write_i32(w)?; // mPhysicalDisplayWidth
            p.write_i32(h)?; // mPhysicalDisplayHeight
            ap::boolean(p, false)?; // mIsRound
            ap::string8(p, None)?; // mSpec
            p.write_f32(1.0)?; // mSpecRatio
            p.write_i32(w)?; // mDisplayWidth
            p.write_i32(h)?; // mDisplayHeight
            p.write_i32(0)?; // mRotation
            p.write_i32(0)?; // mOffsetX
            p.write_i32(0)?; // mOffsetY
            p.write_f32(1.0) // mScale
        })?;
        ap::typed_none(p)?; // layoutLimitedRefreshRate
        ap::typed_none(p)?; // appRequestRenderRefreshRateRange
        p.write_f32(1.0)?; // hdrSdrRatio
        ap::null_array(p)?; // thermalRefreshRateThrottling sparse array
        ap::string8(p, None)?; // thermalBrightnessThrottlingDataId
        ap::boolean(p, true) // canHostTasks
    }

    /// android.hardware.OverlayProperties: Java writes 1 then the native
    /// `android.gui.OverlayProperties` stable-AIDL parcelable (size-prefixed).
    fn write_overlay_properties(p: &mut Parcel) -> Result<()> {
        p.write_i32(1)?; // non-null marker (Java side)
        let start = p.data_position();
        p.write_i32(0)?; // parcelable size, patched below
        p.write_i32(0)?; // combinations: empty array
        p.write_i32(0)?; // supportMixedColorSpaces: false
        p.write_i32(-1)?; // lutProperties: null
        let end = p.data_position();
        p.set_data_position(start);
        p.write_i32((end - start) as i32)?;
        p.set_data_position(end);
        Ok(())
    }
}

impl Service for DisplayService {
    const DESCRIPTOR: &'static str = "android.hardware.display.IDisplayManager";
    const TABLE: &'static [(u32, &'static str)] = codes::IDISPLAYMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "getDisplayInfo" => {
                let id = data.read_i32()?;
                ap::no_exception(reply)?;
                if id == DISPLAY_ID_DEFAULT {
                    ap::typed(reply, |p| self.write_display_info(p))?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getDisplayIds" => {
                let _include_disabled = data.read_i32()?;
                ap::no_exception(reply)?;
                ap::int_array(reply, Some(&[DISPLAY_ID_DEFAULT]))?;
                Ok(true)
            }
            "getPreferredWideGamutColorSpaceId" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // ColorSpace.Named.SRGB
                Ok(true)
            }
            "getOverlaySupport" => {
                ap::no_exception(reply)?;
                Self::write_overlay_properties(reply)?;
                Ok(true)
            }
            "registerCallbackWithEventMask" => {
                let cb: Option<SIBinder> = data.read()?;
                let mask = data.read_i64()?;
                if let Some(cb) = cb {
                    self.callbacks.lock().unwrap().push((cb, mask));
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            "registerCallback" => {
                let cb: Option<SIBinder> = data.read()?;
                if let Some(cb) = cb {
                    self.callbacks.lock().unwrap().push((cb, -1));
                }
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
