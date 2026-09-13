//! Native MediaRecorder audio capture: PipeWire PCM -> host AAC encoder -> app fd.
use super::Service;
use rsbinder::{Parcel, Result, TransactionCode};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
#[derive(Default)]
pub struct MediaRecorder {
    state: Mutex<State>,
}
struct State {
    output: Option<std::fs::File>,
    format: i32,
    encoder: i32,
    prepared: bool,
    capture: Option<Child>,
    encode: Option<Child>,
    rate: u32,
    channels: u32,
    bitrate: u32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            output: None,
            format: 0,
            encoder: 0,
            prepared: false,
            capture: None,
            encode: None,
            rate: 48000,
            channels: 1,
            bitrate: 128000,
        }
    }
}
fn child_setup(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        });
    }
}
fn reap(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    for _ in 0..100 {
        if let Some(s) = child.try_wait()? {
            return Ok(s);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill()?;
    child.wait()
}
impl State {
    fn parameter(&mut self, param: &str) -> bool {
        let Some((key, value)) = param.split_once('=') else {
            return false;
        };
        let Ok(value) = value.parse::<u32>() else {
            return false;
        };
        match key {
            "audio-param-sampling-rate" if (8000..=192000).contains(&value) => self.rate = value,
            "audio-param-number-of-channels" if (1..=2).contains(&value) => self.channels = value,
            "audio-param-encoding-bitrate" if (8000..=512000).contains(&value) => {
                self.bitrate = value
            }
            _ => return false,
        }
        true
    }
    fn stop(&mut self) -> i32 {
        let active = self.capture.is_some();
        if let Some(mut child) = self.capture.take() {
            unsafe {
                libc::kill(child.id() as i32, libc::SIGINT);
            }
            let _ = reap(&mut child);
        }
        let ok = self
            .encode
            .take()
            .map(|mut c| reap(&mut c).is_ok_and(|s| s.success()))
            .unwrap_or(false);
        self.prepared = false;
        if active && ok {
            0
        } else {
            -38
        }
    }
    fn start(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.prepared && self.capture.is_none(),
            "recorder is not prepared"
        );
        let mut out = self
            .output
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no output file"))?
            .try_clone()?;
        if out.metadata()?.is_file() {
            out.set_len(0)?;
            std::io::Seek::seek(&mut out, std::io::SeekFrom::Start(0))?;
        }
        let mut cmd = Command::new("pw-record");
        cmd.args([
            "--raw",
            "--rate",
            &self.rate.to_string(),
            "--channels",
            &self.channels.to_string(),
            "--format",
            "s16",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        child_setup(&mut cmd);
        let mut capture = cmd.spawn()?;
        let mut cmd = Command::new("ffmpeg");
        cmd.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "s16le",
            "-ar",
            &self.rate.to_string(),
            "-ac",
            &self.channels.to_string(),
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            &self.bitrate.to_string(),
        ]);
        if self.format == 6 {
            cmd.args(["-f", "adts"]);
        } else {
            cmd.args(["-f", "mp4", "-movflags", "frag_keyframe+empty_moov"]);
        }
        cmd.arg("pipe:1")
            .stdin(capture.stdout.take().unwrap())
            .stdout(out)
            .stderr(Stdio::null());
        child_setup(&mut cmd);
        match cmd.spawn() {
            Ok(encode) => {
                self.capture = Some(capture);
                self.encode = Some(encode);
                Ok(())
            }
            Err(e) => {
                let _ = capture.kill();
                let _ = capture.wait();
                Err(e.into())
            }
        }
    }
}
impl Drop for State {
    fn drop(&mut self) {
        self.stop();
    }
}
impl Service for MediaRecorder {
    const DESCRIPTOR: &'static str = "android.media.IMediaRecorder";
    // Installed libmedia.so BnMediaRecorder jump table: this image adds native
    // Surface parcel variants at 6 and 23, shifting the historical transaction IDs.
    const TABLE: &'static [(u32, &'static str)] = &[
        (1, "release"),
        (2, "init"),
        (3, "close"),
        (7, "reset"),
        (8, "stop"),
        (9, "start"),
        (10, "prepare"),
        (11, "amplitude"),
        (12, "videoSource"),
        (13, "audioSource"),
        (14, "format"),
        (15, "videoEncoder"),
        (16, "encoder"),
        (17, "output"),
        (21, "parameters"),
        (25, "listener"),
        (26, "clientName"),
        (27, "pause"),
        (28, "resume"),
        (30, "inputDevice"),
        (31, "routedDevices"),
        (32, "audioCallback"),
        (38, "privacy"),
        (39, "getPrivacy"),
    ];
    fn handle(
        &self,
        name: &str,
        _: TransactionCode,
        data: &mut Parcel,
        reply: &mut Parcel,
    ) -> Result<bool> {
        let mut s = self.state.lock().unwrap();
        let status = match name {
            "init" | "listener" | "clientName" | "audioCallback" | "privacy" => 0,
            "audioSource" => {
                if matches!(data.read_i32()?, 0 | 1 | 5 | 6 | 7 | 9) {
                    0
                } else {
                    -22
                }
            }
            "format" => {
                let n = data.read_i32()?;
                if matches!(n, 2 | 6) {
                    s.format = n;
                    0
                } else {
                    -22
                }
            }
            "encoder" => {
                let n = data.read_i32()?;
                if n == 3 {
                    s.encoder = n;
                    0
                } else {
                    -22
                }
            }
            "output" => {
                let fd = data
                    .read_raw_file_descriptor()?
                    .as_fd()
                    .try_clone_to_owned()
                    .map_err(|_| rsbinder::StatusCode::BadValue)?;
                s.output = Some(std::fs::File::from(fd));
                0
            }
            "parameters" => {
                let param = crate::aparcel::read_string8(data)?.unwrap_or_default();
                if s.parameter(&param) {
                    0
                } else {
                    -22
                }
            }
            "prepare" => {
                if s.output.is_some() && s.encoder == 3 && matches!(s.format, 2 | 6) {
                    s.prepared = true;
                    0
                } else {
                    -22
                }
            }
            "start" => match s.start() {
                Ok(()) => {
                    log::info!("recorder: PipeWire capture started");
                    0
                }
                Err(e) => {
                    log::warn!("recorder: {e}");
                    -38
                }
            },
            "stop" => {
                let result = s.stop();
                log::info!("recorder: capture stopped status={result}");
                result
            }
            "release" | "close" | "reset" => {
                s.stop();
                s.output = None;
                0
            }
            "amplitude" => {
                reply.write_i32(0)?;
                -38
            }
            "getPrivacy" => {
                reply.write_i32(1)?;
                0
            }
            "routedDevices" => {
                reply.write_i32(0)?;
                0
            }
            _ => -38,
        };
        log::debug!("recorder: {name} status={status}");
        reply.write_i32(status)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorder_parameters_are_bounded_and_unknown_keys_fail() {
        let mut s = State::default();
        assert!(s.parameter("audio-param-sampling-rate=44100"));
        assert_eq!(s.rate, 44100);
        assert!(!s.parameter("audio-param-number-of-channels=999999"));
        assert!(!s.parameter("audio-param-encoding-bitrate=-1"));
        assert!(!s.parameter("unrecognized=1"));
        assert_eq!(s.channels, 1);
    }
    #[test]
    fn start_requires_prepared_output() {
        assert!(State::default().start().is_err());
    }
}
