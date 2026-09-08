//! Android `Parcel` conventions on top of rsbinder's `Parcel`.
//!
//! rsbinder writes AIDL-style values (UTF-16 strings, i32 booleans). Android's
//! framework parcelables additionally use `writeString8` (UTF-8), typed
//! objects, typed lists, and `-1`-for-null arrays. These helpers mirror the
//! sequences extracted from the image with `tools/dexspec.py`.
use rsbinder::{Parcel, Result};

/// `Parcel.writeString8`: i32 byte length, bytes + NUL, padded to 4; null = -1.
pub fn string8(p: &mut Parcel, s: Option<&str>) -> Result<()> {
    match s {
        None => p.write_i32(-1),
        Some(s) => {
            p.write_i32(s.len() as i32)?;
            let mut bytes = s.as_bytes().to_vec();
            bytes.push(0);
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
            for w in bytes.chunks_exact(4) {
                p.write_u32(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))?;
            }
            Ok(())
        }
    }
}

/// `Parcel.readString8`.
pub fn read_string8(p: &mut Parcel) -> Result<Option<String>> {
    let len = p.read_i32()?;
    if len < 0 {
        return Ok(None);
    }
    let total = (len as usize + 1 + 3) & !3;
    let mut bytes = Vec::with_capacity(total);
    for _ in 0..total / 4 {
        bytes.extend_from_slice(&p.read_u32()?.to_le_bytes());
    }
    bytes.truncate(len as usize);
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// `Parcel.writeString` (UTF-16); null = -1.
pub fn string16(p: &mut Parcel, s: Option<&str>) -> Result<()> {
    p.write(&s)
}

/// `Parcel.writeBoolean`: an i32.
pub fn boolean(p: &mut Parcel, b: bool) -> Result<()> {
    p.write_i32(b as i32)
}

/// `Parcel.writeTypedObject(null)`.
pub fn typed_none(p: &mut Parcel) -> Result<()> {
    p.write_i32(0)
}

/// `Parcel.writeTypedObject(obj)`: 1 then the object.
pub fn typed<F: FnOnce(&mut Parcel) -> Result<()>>(p: &mut Parcel, write: F) -> Result<()> {
    p.write_i32(1)?;
    write(p)
}

/// `writeString8Array`, `writeIntArray`, `writeLongArray`, `writeTypedArray`,
/// `writeTypedList`, `writeSparseArray`, `writeBundle`, `writeMap`: null = -1.
pub fn null_array(p: &mut Parcel) -> Result<()> {
    p.write_i32(-1)
}

pub fn string8_array(p: &mut Parcel, items: Option<&[&str]>) -> Result<()> {
    match items {
        None => null_array(p),
        Some(items) => {
            p.write_i32(items.len() as i32)?;
            for s in items {
                string8(p, Some(s))?;
            }
            Ok(())
        }
    }
}

pub fn int_array(p: &mut Parcel, items: Option<&[i32]>) -> Result<()> {
    match items {
        None => null_array(p),
        Some(items) => {
            p.write_i32(items.len() as i32)?;
            for v in items {
                p.write_i32(*v)?;
            }
            Ok(())
        }
    }
}

pub fn long_array(p: &mut Parcel, items: Option<&[i64]>) -> Result<()> {
    match items {
        None => null_array(p),
        Some(items) => {
            p.write_i32(items.len() as i32)?;
            for v in items {
                p.write_i64(*v)?;
            }
            Ok(())
        }
    }
}

/// An empty typed list / map / typed array (count 0).
pub fn empty_list(p: &mut Parcel) -> Result<()> {
    p.write_i32(0)
}

/// `TextUtils.writeToParcel` for a plain (non-Spanned) CharSequence.
pub fn char_sequence(p: &mut Parcel, s: Option<&str>) -> Result<()> {
    p.write_i32(1)?;
    string8(p, s)
}

/// `Parcelling.BuiltIn.ForBoolean`: null = 1, true = -1, false = 0.
pub fn boxed_boolean(p: &mut Parcel, b: Option<bool>) -> Result<()> {
    p.write_i32(match b {
        None => 1,
        Some(true) => -1,
        Some(false) => 0,
    })
}

/// Reply header for a successful call (`Parcel.writeNoException`).
pub fn no_exception(p: &mut Parcel) -> Result<()> {
    p.write_i32(0)
}
