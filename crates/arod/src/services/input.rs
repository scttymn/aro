//! `input`: android.hardware.input.IInputManager. No devices are reported yet;
//! pointer/keyboard events reach the app through the window's input channel
//! (M3), so this only has to keep InputManagerGlobal alive.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct InputService;

pub const VIRTUAL_KEYBOARD: i32 = -1;

/// A C string as libbinder's writeCString: bytes + NUL, padded to 4 (no length prefix).
#[allow(dead_code)]
fn cstring(p: &mut Parcel, s: &str) -> Result<()> {
    let mut b = s.as_bytes().to_vec();
    b.push(0);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    for w in b.chunks_exact(4) {
        p.write_u32(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))?;
    }
    Ok(())
}

/// android.view.InputDevice for the virtual keyboard (id -1), the device every
/// Android system exposes. Its key character map is empty: the framework only
/// needs the object to exist to prepare menus and translate shortcuts.
fn write_virtual_keyboard(p: &mut Parcel) -> Result<()> {
    // KeyCharacterMap (JNI: deviceId, bool has-map, then KeyCharacterMap::writeToParcel)
    p.write_i32(VIRTUAL_KEYBOARD)?;
    p.write_i32(1)?; // map present
    ap::string8(p, Some("/system/usr/keychars/Virtual.kcm"))?; // mLoadFileName (String8 in android17)
    p.write_i32(4)?; // KeyboardType::FULL
    p.write_i32(0)?; // mLayoutOverlayApplied
    p.write_i32(0)?; // keys
    p.write_i32(0)?; // key remapping
    p.write_i32(0)?; // keys by scan code
    p.write_i32(0)?; // keys by usage code
    // InputDevice fields
    p.write_i32(VIRTUAL_KEYBOARD)?; // mId
    p.write_i32(1)?; // mGeneration
    p.write_i32(0)?; // mControllerNumber
    ap::string16(p, Some("Virtual"))?; // mName
    p.write_i32(0)?; // mVendorId
    p.write_i32(0)?; // mProductId
    p.write_i32(0)?; // mDeviceBus
    ap::string16(p, Some("aro-virtual-keyboard"))?; // mDescriptor
    p.write_i32(0)?; // mIsExternal
    p.write_i32(0)?; // mIsVirtualDevice
    p.write_i32(0x101 | 0x201)?; // mSources: KEYBOARD | DPAD
    p.write_i32(2)?; // mKeyboardType: ALPHABETIC
    ap::string8(p, None)?; // mKeyboardLanguageTag
    ap::string8(p, None)?; // mKeyboardLayoutType
    p.write_i32(0)?; // mHasVibrator
    p.write_i32(0)?; // mHasMicrophone
    p.write_i32(0)?; // mHasSensor
    p.write_i32(0)?; // mHasBattery
    p.write_i32(-1)?; // HostUsiVersion.majorVersion
    p.write_i32(-1)?; // HostUsiVersion.minorVersion
    p.write_i32(-1)?; // mAssociatedDisplayId: INVALID_DISPLAY
    p.write_i32(1)?; // mEnabled
    p.write_i32(0)?; // motion ranges
    p.write_i32(0)?; // ViewBehavior.mShouldSmoothScroll (writeBoolean)
    p.write_i32(-1)?; // ViewBehavior.mPrimaryDirectionalMotionAxis
    Ok(())
}

impl Service for InputService {
    const DESCRIPTOR: &'static str = "android.hardware.input.IInputManager";
    const TABLE: &'static [(u32, &'static str)] = super::input_codes::IINPUTMANAGER;

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        log::info!("input: {name}");
        match name {
            "getInputDeviceIds" => {
                ap::no_exception(reply)?;
                ap::int_array(reply, Some(&[VIRTUAL_KEYBOARD]))?;
                Ok(true)
            }
            "getInputDevice" => {
                let id = _data.read_i32()?;
                ap::no_exception(reply)?;
                if id == VIRTUAL_KEYBOARD {
                    reply.write_i32(1)?;
                    write_virtual_keyboard(reply)?;
                } else {
                    ap::typed_none(reply)?;
                }
                Ok(true)
            }
            "getTouchCalibrationForInputDevice" | "getKeyboardLayout" | "getCurrentKeyboardLayoutForInputDevice" | "getKeyboardLayoutForInputDevice" | "getInputDeviceBluetoothAddress" | "getBatteryState" | "getSensorList" | "getLightsList" | "getLightState" | "getVelocityTrackerStrategy" | "getInputDeviceVibrator" | "getInputDeviceVibratorIds" | "getKeyboardLayoutsForInputDevice" | "getKeyboardLayouts" | "getKeyboardLayoutListForInputDevice" | "getMouseScrollingAcceleration" => {
                ap::no_exception(reply)?;
                ap::typed_none(reply)?;
                Ok(true)
            }
            "isInputDeviceEnabled" | "hasKeys" | "isMouse" | "injectInputEvent" | "injectInputEventToTarget" | "isPointerAccelerationEnabled" | "isTouchpadTapToClickEnabled" | "isStylusPointerIconEnabled" | "isVibrating" | "isInTabletMode" | "isMicMuted" | "registerLightListener" | "isTouchpadRightClickZoneEnabled" | "isTouchpadTapDraggingEnabled" | "isMouseReverseVerticalScrollingEnabled" | "isMouseSwapPrimaryButtonEnabled" | "isMouseScrollingAccelerationEnabled" | "areMouseAccelerationsEnabled" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
                Ok(true)
            }
            "getMousePointerSpeed" | "getHostUsiVersion" | "getMaximumObscuringOpacityForTouch" | "getTouchpadPointerSpeed" | "getMouseScrollingSpeed" | "getMouseAccelerationValue" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            n if n.starts_with("register") || n.starts_with("unregister") || n.starts_with("set") || n.starts_with("remove") || n.starts_with("add") || n.starts_with("notify") || n.starts_with("request") || n.starts_with("cancel") || n.starts_with("clear") || n == "vibrate" || n == "vibrateCombined" || n == "cancelVibrate" || n == "monitorGestureInput" => {
                ap::no_exception(reply)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
