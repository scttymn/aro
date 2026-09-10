//! `media.audio_policy`: android.media.IAudioPolicyService.
//! Minimal stub so AudioSystem doesn't block for 10 seconds waiting for AudioPolicy.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct AudioPolicyService;

impl Service for AudioPolicyService {
    const DESCRIPTOR: &'static str = "android.media.IAudioPolicyService";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "onNewAudioModulesAvailable"),
        (2, "setDeviceConnectionState"),
        (3, "getDeviceConnectionState"),
        (4, "handleDeviceConfigChange"),
        (5, "setPhoneState"),
        (6, "setForceUse"),
        (7, "getForceUse"),
        (8, "getOutput"),
        (9, "getOutputForAttr"),
        (10, "startOutput"),
        (11, "stopOutput"),
        (12, "releaseOutput"),
        (13, "getInputForAttr"),
        (14, "startInput"),
        (15, "stopInput"),
        (16, "releaseInput"),
        (17, "initStreamVolume"),
        (18, "setStreamVolumeIndex"),
        (19, "getStreamVolumeIndex"),
        (20, "setVolumeIndexForAttributes"),
        (21, "getVolumeIndexForAttributes"),
        (22, "getMaxVolumeIndexForAttributes"),
        (23, "getMinVolumeIndexForAttributes"),
        (24, "getStrategyForStream"),
        (25, "getDevicesForAttributes"),
        (26, "getOutputForEffect"),
        (27, "registerEffect"),
        (28, "unregisterEffect"),
        (29, "setEffectEnabled"),
        (30, "moveEffectsToIo"),
        (31, "isStreamActive"),
        (32, "isStreamActiveRemotely"),
        (33, "isSourceActive"),
        (34, "queryDefaultPreProcessing"),
        (35, "addSourceDefaultEffect"),
        (36, "addStreamDefaultEffect"),
        (37, "removeSourceDefaultEffect"),
        (38, "removeStreamDefaultEffect"),
        (39, "setSupportedSystemUsages"),
        (40, "setAllowedCapturePolicy"),
        (41, "getOffloadSupport"),
        (42, "isDirectOutputSupported"),
        (43, "listAudioPorts"),
        (44, "listDeclaredDevicePorts"),
        (45, "getAudioPort"),
        (46, "createAudioPatch"),
        (47, "releaseAudioPatch"),
        (48, "listAudioPatches"),
        (49, "setAudioPortConfig"),
        (50, "registerClient"),
        (51, "setAudioPortCallbacksEnabled"),
        (52, "setAudioVolumeGroupCallbacksEnabled"),
        (53, "acquireSoundTriggerSession"),
        (54, "releaseSoundTriggerSession"),
        (55, "getPhoneState"),
        (56, "registerPolicyMixes"),
        (57, "setUidDeviceAffinities"),
        (58, "removeUidDeviceAffinities"),
        (59, "setUserIdDeviceAffinities"),
        (60, "removeUserIdDeviceAffinities"),
        (61, "startAudioSource"),
        (62, "stopAudioSource"),
        (63, "setMasterMono"),
        (64, "getMasterMono"),
        (65, "getStreamVolumeDB"),
        (66, "getSurroundFormats"),
        (67, "getReportedSurroundFormats"),
        (68, "getHwOffloadFormatsSupportedForBluetoothMedia"),
        (69, "setSurroundFormatEnabled"),
        (70, "setAssistantServicesUids"),
        (71, "setActiveAssistantServicesUids"),
        (72, "setA11yServicesUids"),
        (73, "setCurrentImeUid"),
        (74, "isHapticPlaybackSupported"),
        (75, "isUltrasoundSupported"),
        (76, "isHotwordStreamSupported"),
        (77, "listAudioProductStrategies"),
        (78, "getProductStrategyFromAudioAttributes"),
        (79, "listAudioVolumeGroups"),
        (80, "getVolumeGroupFromAudioAttributes"),
        (81, "setRttEnabled"),
        (82, "isCallScreenModeSupported"),
        (83, "setDevicesRoleForStrategy"),
        (84, "removeDevicesRoleForStrategy"),
        (85, "clearDevicesRoleForStrategy"),
        (86, "getDevicesForRoleAndStrategy"),
        (87, "setDevicesRoleForCapturePreset"),
        (88, "addDevicesRoleForCapturePreset"),
        (89, "removeDevicesRoleForCapturePreset"),
        (90, "clearDevicesRoleForCapturePreset"),
        (91, "getDevicesForRoleAndCapturePreset"),
        (92, "registerSoundTriggerCaptureStateListener"),
        (93, "getSpatializer"),
        (94, "canBeSpatialized"),
        (95, "getDirectPlaybackSupport"),
        (96, "getDirectProfilesForAttributes"),
        (97, "getSupportedMixerAttributes"),
        (98, "setPreferredMixerAttributes"),
        (99, "getPreferredMixerAttributes"),
        (100, "clearPreferredMixerAttributes"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "onNewAudioModulesAvailable" => {
                Ok(true) // oneway
            }
            "registerClient" => {
                log::info!("audio_policy: registerClient");
                ap::no_exception(reply)?;
                Ok(true)
            }
            "listAudioProductStrategies" | "listAudioVolumeGroups" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // 0 elements
                Ok(true)
            }
            "getStrategyForStream" | "getVolumeGroupFromAudioAttributes" | "getProductStrategyFromAudioAttributes" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "getDevicesForAttributes" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?; // 0 devices
                Ok(true)
            }
            "getStreamVolumeIndex" | "getVolumeIndexForAttributes" | "getMaxVolumeIndexForAttributes" => {
                ap::no_exception(reply)?;
                reply.write_i32(100)?;
                Ok(true)
            }
            "getMinVolumeIndexForAttributes" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            "isStreamActive" | "isStreamActiveRemotely" | "isSourceActive" => {
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
            _ => {
                log::debug!("audio_policy: {name} (stub)");
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}
