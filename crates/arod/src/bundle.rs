//! Best-effort reader for android.os.Bundle as it appears inside a parcel:
//! `[i32 length][i32 magic 'BNDL'][bool hasIntent][i32 count]` then `count`
//! entries of `String16 key` + `Parcel.writeValue` (i32 type + payload).
//! Used to lift human-readable fields (notification title/text) out of
//! parcelables we don't otherwise model. Stops at the first value type it
//! cannot size; container types carry a length prefix and are skipped.
use std::collections::BTreeMap;

pub const BUNDLE_MAGIC: i32 = 0x4c44_4e42;
pub const BUNDLE_MAGIC_NATIVE: i32 = 0x4c44_4e44;

#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Str(String),
    Strs(Vec<String>),
    Int(i32),
    Long(i64),
    Bool(bool),
    Float(f32),
    Double(f64),
    #[allow(dead_code)]
    Skipped(i32),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_strs(&self) -> Option<&[String]> {
        match self {
            Value::Strs(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i as i64),
            Value::Long(i) => Some(*i),
            Value::Str(s) => s.parse().ok(),
            _ => None,
        }
    }
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn i32(&mut self) -> Option<i32> {
        let v = self.b.get(self.p..self.p + 4)?;
        self.p += 4;
        Some(i32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn i64(&mut self) -> Option<i64> {
        let v = self.b.get(self.p..self.p + 8)?;
        self.p += 8;
        Some(i64::from_le_bytes(v.try_into().ok()?))
    }
    /// String16: i32 len (or -1), UTF-16 units + NUL, padded to 4.
    fn string16(&mut self) -> Option<Option<String>> {
        let len = self.i32()?;
        if len < 0 {
            return Some(None);
        }
        let len = len as usize;
        let bytes = ((len + 1) * 2 + 3) & !3;
        let v = self.b.get(self.p..self.p + bytes)?;
        self.p += bytes;
        let units: Vec<u16> = (0..len).map(|k| u16::from_le_bytes([v[k * 2], v[k * 2 + 1]])).collect();
        Some(Some(String::from_utf16_lossy(&units)))
    }
    /// String8: i32 len (or -1), bytes + NUL, padded to 4.
    fn string8(&mut self) -> Option<Option<String>> {
        let len = self.i32()?;
        if len < 0 {
            return Some(None);
        }
        let len = len as usize;
        let bytes = (len + 1 + 3) & !3;
        let v = self.b.get(self.p..self.p + bytes)?;
        self.p += bytes;
        Some(Some(String::from_utf8_lossy(&v[..len]).into_owned()))
    }
}

fn is_length_prefixed(t: i32) -> bool {
    // VAL_MAP, VAL_PARCELABLE, VAL_LIST, VAL_SPARSEARRAY, VAL_PARCELABLEARRAY, VAL_OBJECTARRAY, VAL_SERIALIZABLE
    matches!(t, 2 | 4 | 11 | 12 | 16 | 17 | 21)
}

/// Parse one bundle whose magic sits at `magic_at`. Returns its entries (as far as they could be read).
pub fn parse_at(bytes: &[u8], magic_at: usize) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let mut c = Cur { b: bytes, p: magic_at + 4 };
    // Newer bundles carry a hasIntent bool between magic and count; older ones don't.
    let save = c.p;
    let first = match c.i32() { Some(v) => v, None => return out };
    let count = if first == 0 || first == 1 {
        match c.i32() { Some(v) => v, None => return out }
    } else {
        first
    };
    if !(0..=256).contains(&count) {
        // Try the old layout.
        c.p = save;
        let Some(n) = c.i32() else { return out };
        if !(0..=256).contains(&n) {
            return out;
        }
    }
    for _ in 0..count {
        let Some(Some(key)) = c.string16() else { break };
        let Some(t) = c.i32() else { break };
        let v = if is_length_prefixed(t) {
            let Some(len) = c.i32() else { break };
            c.p += len.max(0) as usize;
            Value::Skipped(t)
        } else {
            match t {
                -1 => Value::Null,
                0 => match c.string16() { Some(s) => Value::Str(s.unwrap_or_default()), None => break },
                1 => match c.i32() { Some(i) => Value::Int(i), None => break },
                6 => match c.i64() { Some(i) => Value::Long(i), None => break },
                7 => match c.i32() { Some(i) => Value::Float(f32::from_bits(i as u32)), None => break },
                8 => match c.i64() { Some(i) => Value::Double(f64::from_bits(i as u64)), None => break },
                9 => match c.i32() { Some(i) => Value::Bool(i != 0), None => break },
                10 => {
                    // CharSequence: TextUtils.writeToParcel: 0 + string8 (plain) | 1 + string8 + spans…
                    let Some(kind) = c.i32() else { break };
                    let Some(s) = c.string8() else { break };
                    let v = Value::Str(s.unwrap_or_default());
                    if kind != 0 {
                        out.insert(key, v);
                        break; // spans follow with formats we don't size; keep what we have
                    }
                    v
                }
                3 => {
                    // nested Bundle: [len][magic]…
                    let Some(len) = c.i32() else { break };
                    c.p += len.max(0) as usize;
                    Value::Skipped(t)
                }
                14 => {
                    // String[]: n + String16s
                    let Some(n) = c.i32() else { break };
                    if !(0..=4096).contains(&n) {
                        break;
                    }
                    let mut v = Vec::with_capacity(n.max(0) as usize);
                    let mut ok = true;
                    for _ in 0..n.max(0) {
                        match c.string16() {
                            Some(s) => v.push(s.unwrap_or_default()),
                            None => {
                                ok = false;
                                break;
                            }
                        }
                    }
                    if !ok {
                        break;
                    }
                    Value::Strs(v)
                }
                _ => {
                    out.insert(key, Value::Skipped(t));
                    break;
                }
            }
        };
        out.insert(key, v);
    }
    out
}

/// Parse `bytes` as a single Bundle. Accepts a payload that starts at the
/// magic, or a 4-byte length prefix then magic (as written on the wire).
pub fn parse(bytes: &[u8]) -> BTreeMap<String, Value> {
    if bytes.len() >= 4 {
        let m = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        if m == BUNDLE_MAGIC || m == BUNDLE_MAGIC_NATIVE {
            return parse_at(bytes, 0);
        }
        if bytes.len() >= 8 {
            let m2 = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            if m2 == BUNDLE_MAGIC || m2 == BUNDLE_MAGIC_NATIVE {
                return parse_at(bytes, 4);
            }
        }
    }
    find_all(bytes).into_iter().next().unwrap_or_default()
}

/// Every bundle in `bytes`, in order of appearance.
pub fn find_all(bytes: &[u8]) -> Vec<BTreeMap<String, Value>> {
    let mut v = Vec::new();
    let mut i = 4;
    while i + 4 <= bytes.len() {
        let m = i32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        if m == BUNDLE_MAGIC || m == BUNDLE_MAGIC_NATIVE {
            v.push(parse_at(bytes, i));
        }
        i += 4;
    }
    v
}

/// Write an android.os.Bundle containing string key-value pairs.
/// If `map` is empty, writes an empty bundle (length 0).
/// If a value is `None`, writes a null string entry.
pub fn write_string_bundle(p: &mut rsbinder::Parcel, map: &[(&str, Option<&str>)]) -> rsbinder::Result<()> {
    if map.is_empty() {
        return p.write_i32(0);
    }
    let len_pos = p.data_position();
    p.write_i32(-1)?; // placeholder for length
    p.write_i32(BUNDLE_MAGIC)?;
    let start_pos = p.data_position();

    p.write_i32(map.len() as i32)?;
    for (k, v) in map {
        crate::aparcel::string16(p, Some(k))?;
        match v {
            Some(s) => {
                p.write_i32(0)?; // VAL_STRING = 0
                crate::aparcel::string16(p, Some(s))?;
            }
            None => {
                p.write_i32(-1)?; // VAL_NULL = -1
            }
        }
    }
    let end_pos = p.data_position();
    let payload_len = (end_pos - start_pos) as i32;
    p.set_data_position(len_pos);
    p.write_i32(payload_len)?;
    p.set_data_position(end_pos);
    crate::aparcel::boolean(p, false)?; // hasIntent = false
    Ok(())
}

