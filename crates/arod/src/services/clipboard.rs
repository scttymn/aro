//! Text ClipData bridged to the host Wayland clipboard via wl-clipboard.
use super::{codes, Service};
use crate::aparcel as ap;
use rsbinder::{Parcel, Result, SIBinder, StatusCode, TransactionCode, FLAG_ONEWAY};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::Mutex,
};

#[derive(Default)]
pub struct ClipboardService {
    listeners: Mutex<Vec<SIBinder>>,
}

fn host_text() -> Option<String> {
    let output = Command::new("timeout")
        .args([
            "--kill-after=1s",
            "2s",
            "wl-paste",
            "--no-newline",
            "--type",
            "text",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

fn write_host(text: Option<&str>) -> std::io::Result<()> {
    let mut command = Command::new("timeout");
    command.args(["--kill-after=1s", "2s", "wl-copy"]);
    if text.is_some() {
        command.args(["--type", "text/plain;charset=utf-8"]);
    } else {
        command.arg("--clear");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let write_result = if let (Some(mut stdin), Some(text)) = (child.stdin.take(), text) {
        stdin.write_all(text.as_bytes())
    } else {
        Ok(())
    };
    let status = child.wait()?;
    write_result?;
    if !status.success() {
        return Err(std::io::Error::other("host clipboard write failed"));
    }
    Ok(())
}

fn skip_bundle(p: &mut Parcel, has_intent_flag: bool) -> Result<()> {
    let len = p.read_i32()?;
    if len > 0 {
        let end = p
            .data_position()
            .checked_add(4 + ((len as usize + 3) & !3))
            .ok_or(StatusCode::BadValue)?;
        if end > p.data_size() {
            return Err(StatusCode::NotEnoughData);
        }
        p.set_data_position(end);
        if has_intent_flag {
            p.read_i32()?;
        }
    }
    Ok(())
}

/// Read the first textual item. Item styles, HTML, URIs and additional items
/// are not exported; the host gets plain UTF-8 text.
fn read_clip_text(p: &mut Parcel) -> Result<Option<String>> {
    if p.read_i32()? == 0 {
        return Ok(None);
    } // typed ClipData
    if p.read_i32()? != 1 {
        return Err(StatusCode::BadValue);
    } // plain description label
    ap::read_string8(p)?;
    let mime_count = p.read_i32()?;
    if !(0..=128).contains(&mime_count) {
        return Err(StatusCode::BadValue);
    }
    for _ in 0..mime_count {
        let _: Option<String> = p.read()?;
    }
    skip_bundle(p, false)?; // PersistableBundle extras
    p.read_i64()?; // timestamp
    p.read_i32()?; // styled text
    p.read_i32()?; // classification status
    skip_bundle(p, true)?; // confidence Bundle
    for _ in 0..3 {
        p.read_i32()?;
    } // content URI, remote, excluded from remote
    let _: Option<String> = p.read()?; // source device name
    if p.read_i32()? != 0 {
        return Err(StatusCode::BadValue);
    } // bitmap icon unsupported
    if p.read_i32()? <= 0 {
        return Ok(None);
    }
    let kind = p.read_i32()?; // TextUtils: plain=1, spanned=0
    if kind != 0 && kind != 1 {
        return Err(StatusCode::BadValue);
    }
    // Both forms begin with the same text; remaining span/item fields need
    // no decoding because this is the last incoming field we use.
    ap::read_string8(p)
}

fn write_description(p: &mut Parcel) -> Result<()> {
    ap::char_sequence(p, None)?;
    p.write_i32(1)?; // MIME types
    ap::string16(p, Some("text/plain"))?;
    p.write_i32(-1)?; // PersistableBundle extras
    p.write_i64(0)?; // timestamp unavailable from Wayland
    p.write_i32(0)?; // isStyledText
    p.write_i32(2)?; // CLASSIFICATION_NOT_PERFORMED (CP41 image)
    p.write_i32(0)?; // empty confidence Bundle
    p.write_i32(0)?; // contains content URI
    p.write_i32(0)?; // remote clipboard
    p.write_i32(0)?; // excluded from remote clipboard
    ap::string16(p, None) // source device name
}

fn write_clip(p: &mut Parcel, text: &str) -> Result<()> {
    write_description(p)?;
    p.write_i32(0)?; // icon absent
    p.write_i32(1)?; // item count
    ap::char_sequence(p, Some(text))?;
    ap::string8(p, None)?; // HTML
    for _ in 0..5 {
        ap::typed_none(p)?;
    } // Intent, IntentSender, URI, ActivityInfo, TextLinks
    Ok(())
}

impl ClipboardService {
    fn changed(&self) {
        let listeners = self.listeners.lock().unwrap().clone();
        for listener in listeners {
            if let Some(proxy) = listener.as_proxy() {
                if let Ok(data) = proxy.prepare_transact(true) {
                    if proxy.submit_transact(1, &data, FLAG_ONEWAY).is_err() {
                        self.listeners.lock().unwrap().retain(|b| b != &listener);
                    }
                }
            }
        }
    }
}

impl Service for ClipboardService {
    const DESCRIPTOR: &'static str = "android.content.IClipboard";
    const TABLE: &'static [(u32, &'static str)] = codes::ICLIPBOARD;
    fn handle(
        &self,
        name: &str,
        _: TransactionCode,
        data: &mut Parcel,
        reply: &mut Parcel,
    ) -> Result<bool> {
        match name {
            "setPrimaryClip" | "setPrimaryClipAsPackage" => {
                if let Some(text) = read_clip_text(data)? {
                    write_host(Some(&text)).map_err(|e| {
                        log::warn!("clipboard: {e}");
                        StatusCode::Unknown
                    })?;
                    self.changed();
                }
                ap::no_exception(reply)?;
            }
            "clearPrimaryClip" => {
                write_host(None).map_err(|_| StatusCode::Unknown)?;
                self.changed();
                ap::no_exception(reply)?;
            }
            "getPrimaryClip" | "getPrimaryClipDescription" => {
                let text = host_text();
                ap::no_exception(reply)?;
                if let Some(text) = text {
                    ap::typed(reply, |p| {
                        if name == "getPrimaryClip" {
                            write_clip(p, &text)
                        } else {
                            write_description(p)
                        }
                    })?;
                } else {
                    ap::typed_none(reply)?;
                }
            }
            "hasPrimaryClip" | "hasClipboardText" => {
                let text = host_text();
                ap::no_exception(reply)?;
                ap::boolean(
                    reply,
                    text.is_some_and(|s| name == "hasPrimaryClip" || !s.is_empty()),
                )?;
            }
            "addPrimaryClipChangedListener" | "removePrimaryClipChangedListener" => {
                if let Some(listener) = data.read::<Option<SIBinder>>()? {
                    let mut listeners = self.listeners.lock().unwrap();
                    listeners.retain(|b| b != &listener);
                    if name == "addPrimaryClipChangedListener" {
                        listeners.push(listener);
                    }
                }
                ap::no_exception(reply)?;
            }
            "getPrimaryClipSource" => {
                ap::no_exception(reply)?;
                ap::string16(reply, None)?;
            }
            "areClipboardAccessNotificationsEnabledForUser" => {
                ap::no_exception(reply)?;
                ap::boolean(reply, false)?;
            }
            "setClipboardAccessNotificationsEnabledForUser" | "notifyUserAuthorizedClipAccess" => {
                ap::no_exception(reply)?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipdata_roundtrips_unicode_newlines_and_empty_text() {
        for text in ["", "ARO café 🙂\nsecond line"] {
            let mut parcel = Parcel::new();
            ap::typed(&mut parcel, |p| write_clip(p, text)).unwrap();
            parcel.set_data_position(0);
            assert_eq!(read_clip_text(&mut parcel).unwrap().as_deref(), Some(text));
        }
    }
    #[test]
    fn malformed_clip_does_not_become_clipboard_text() {
        let mut parcel = Parcel::new();
        parcel.write_i32(1).unwrap();
        parcel.write_i32(1).unwrap();
        ap::string8(&mut parcel, None).unwrap();
        parcel.write_i32(i32::MAX).unwrap();
        parcel.set_data_position(0);
        assert!(read_clip_text(&mut parcel).is_err());
    }
}
