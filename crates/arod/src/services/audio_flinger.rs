//! `media.audio_flinger`: android.media.IAudioFlingerService.
//! Minimal stub so AudioSystem doesn't block for 10 seconds waiting for AudioFlinger.
use super::Service;
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, TransactionCode};

pub struct AudioFlingerService;

impl Service for AudioFlingerService {
    const DESCRIPTOR: &'static str = "android.media.IAudioFlingerService";
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "createTrack"),
        (2, "createRecord"),
        (3, "sampleRate"),
        (4, "format"),
        (5, "frameCount"),
        (6, "latency"),
        (7, "setMasterVolume"),
        (8, "setMasterMute"),
        (9, "masterVolume"),
        (10, "masterMute"),
        (11, "setMasterBalance"),
        (12, "getMasterBalance"),
        (13, "setStreamVolume"),
        (14, "setStreamMute"),
        (15, "streamVolume"),
        (16, "streamMute"),
        (17, "setMode"),
        (18, "setMicMute"),
        (19, "getMicMute"),
        (20, "setRecordSilenced"),
        (21, "setParameters"),
        (22, "registerClient"),
        (23, "getInputBufferSize"),
        (24, "openOutput"),
        (25, "openDuplicateOutput"),
        (26, "closeOutput"),
        (27, "suspendOutput"),
        (28, "restoreOutput"),
        (29, "openInput"),
        (30, "closeInput"),
        (31, "setVoiceVolume"),
        (32, "getRenderPosition"),
        (33, "getInputFramesLost"),
        (34, "newAudioUniqueId"),
        (35, "acquireAudioSessionId"),
        (36, "releaseAudioSessionId"),
        (37, "queryNumberEffects"),
        (38, "queryEffect"),
        (39, "getEffectDescriptor"),
        (40, "createEffect"),
        (41, "moveEffects"),
        (42, "setEffectSuspended"),
        (43, "loadHwModule"),
        (44, "getPrimaryOutputSamplingRate"),
        (45, "getPrimaryOutputFrameCount"),
        (46, "setLowRamDevice"),
        (47, "getAudioPort"),
        (48, "createAudioPatch"),
        (49, "releaseAudioPatch"),
        (50, "listAudioPatches"),
        (51, "setAudioPortConfig"),
        (52, "getAudioHwSyncForSession"),
        (53, "systemReady"),
        (54, "audioPolicyReady"),
        (55, "frameCountHAL"),
        (56, "getMicrophones"),
        (57, "setAudioHalPids"),
        (58, "setVibratorInfos"),
        (59, "updateSecondaryOutputs"),
        (60, "getMmapPolicyInfos"),
        (61, "getAAudioMixerBurstCount"),
        (62, "getAAudioHardwareBurstMinUsec"),
        (63, "setDeviceConnectedState"),
        (64, "setSimulateDeviceConnections"),
        (65, "setRequestedLatencyMode"),
        (66, "getSupportedLatencyModes"),
        (67, "supportsBluetoothVariableLatency"),
        (68, "setBluetoothVariableLatencyEnabled"),
        (69, "isBluetoothVariableLatencyEnabled"),
        (70, "getSoundDoseInterface"),
        (71, "invalidateTracks"),
        (72, "getAudioPolicyConfig"),
    ];

    fn handle(&self, name: &str, _code: TransactionCode, _data: &mut Parcel, reply: &mut Parcel) -> Result<bool> {
        match name {
            "registerClient" => {
                log::info!("audio_flinger: registerClient");
                ap::no_exception(reply)?;
                Ok(true)
            }
            "getPrimaryOutputSamplingRate" => {
                ap::no_exception(reply)?;
                reply.write_i32(48000)?;
                Ok(true)
            }
            "getPrimaryOutputFrameCount" => {
                ap::no_exception(reply)?;
                reply.write_i64(1024)?;
                Ok(true)
            }
            "masterVolume" | "streamVolume" => {
                ap::no_exception(reply)?;
                reply.write_f32(1.0)?;
                Ok(true)
            }
            "newAudioUniqueId" => {
                ap::no_exception(reply)?;
                reply.write_i32(1)?;
                Ok(true)
            }
            "sampleRate" => {
                ap::no_exception(reply)?;
                reply.write_i32(48000)?;
                Ok(true)
            }
            "frameCount" => {
                ap::no_exception(reply)?;
                reply.write_i64(1024)?;
                Ok(true)
            }
            "latency" => {
                ap::no_exception(reply)?;
                reply.write_i32(10)?;
                Ok(true)
            }
            "systemReady" | "audioPolicyReady" => {
                Ok(true) // oneway
            }
            _ => {
                log::debug!("audio_flinger: {name} (stub)");
                ap::no_exception(reply)?;
                reply.write_i32(0)?;
                Ok(true)
            }
        }
    }
}
