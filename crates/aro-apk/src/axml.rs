//! Android binary XML (AXML) reader, as produced by aapt/aapt2 for
//! AndroidManifest.xml. Chunk layout follows `ResourceTypes.h`.
use anyhow::{bail, Result};

const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_TYPE: u16 = 0x0003;
const RES_XML_START_NAMESPACE_TYPE: u16 = 0x0100;
const RES_XML_END_NAMESPACE_TYPE: u16 = 0x0101;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const RES_XML_END_ELEMENT_TYPE: u16 = 0x0103;
const RES_XML_RESOURCE_MAP_TYPE: u16 = 0x0180;

pub const TYPE_REFERENCE: u8 = 0x01;
pub const TYPE_STRING: u8 = 0x03;
pub const TYPE_INT_DEC: u8 = 0x10;
pub const TYPE_INT_HEX: u8 = 0x11;
pub const TYPE_INT_BOOLEAN: u8 = 0x12;

#[derive(Clone, Debug)]
pub enum Value {
    Str(String),
    Reference(u32),
    Int(i32),
    Bool(bool),
    Other(u8, u32),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Reference(r) => Some(*r as i32),
            Value::Bool(b) => Some(*b as i32),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    /// android:attr resource id, when the compiler recorded one (0 otherwise).
    pub res_id: u32,
    pub value: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<Attribute>,
    pub children: Vec<Element>,
}

impl Element {
    /// Attribute by android attr resource id, or by name when the id is absent.
    pub fn attr(&self, res_id: u32, name: &str) -> Option<&Value> {
        self.attrs.iter().find(|a| (res_id != 0 && a.res_id == res_id) || a.name == name).map(|a| &a.value)
    }
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn parse_string_pool(chunk: &[u8]) -> Result<Vec<String>> {
    let count = u32_at(chunk, 8) as usize;
    let flags = u32_at(chunk, 16);
    let strings_start = u32_at(chunk, 20) as usize;
    let utf8 = flags & (1 << 8) != 0;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let off = strings_start + u32_at(chunk, 28 + i * 4) as usize;
        if off >= chunk.len() {
            out.push(String::new());
            continue;
        }
        if utf8 {
            // u8/u16 varint char count, then byte count, then bytes.
            let mut p = off;
            let (mut n, hi) = (chunk[p] as usize, chunk[p] & 0x80 != 0);
            p += 1;
            if hi {
                n = ((n & 0x7f) << 8) | chunk[p] as usize;
                p += 1;
            }
            let _ = n;
            let (mut len, hi) = (chunk[p] as usize, chunk[p] & 0x80 != 0);
            p += 1;
            if hi {
                len = ((len & 0x7f) << 8) | chunk[p] as usize;
                p += 1;
            }
            let end = (p + len).min(chunk.len());
            out.push(String::from_utf8_lossy(&chunk[p..end]).into_owned());
        } else {
            let mut p = off;
            let mut len = u16_at(chunk, p) as usize;
            p += 2;
            if len & 0x8000 != 0 {
                len = ((len & 0x7fff) << 16) | u16_at(chunk, p) as usize;
                p += 2;
            }
            let mut units = Vec::with_capacity(len);
            for k in 0..len {
                if p + k * 2 + 1 >= chunk.len() {
                    break;
                }
                units.push(u16_at(chunk, p + k * 2));
            }
            out.push(String::from_utf16_lossy(&units));
        }
    }
    Ok(out)
}

/// Parse an AXML document into its root element.
pub fn parse(data: &[u8]) -> Result<Element> {
    if data.len() < 8 || u16_at(data, 0) != RES_XML_TYPE {
        bail!("not a binary XML document");
    }
    let header_size = u16_at(data, 2) as usize;
    let mut pos = header_size;
    let mut strings: Vec<String> = Vec::new();
    let mut res_map: Vec<u32> = Vec::new();
    let mut stack: Vec<Element> = vec![Element { name: "<root>".into(), ..Default::default() }];

    while pos + 8 <= data.len() {
        let ty = u16_at(data, pos);
        let hsize = u16_at(data, pos + 2) as usize;
        let size = u32_at(data, pos + 4) as usize;
        if size < 8 || pos + size > data.len() {
            bail!("bad chunk at {pos}");
        }
        let chunk = &data[pos..pos + size];
        match ty {
            RES_STRING_POOL_TYPE => strings = parse_string_pool(chunk)?,
            RES_XML_RESOURCE_MAP_TYPE => {
                res_map = (hsize..size).step_by(4).map(|i| u32_at(chunk, i)).collect();
            }
            RES_XML_START_NAMESPACE_TYPE | RES_XML_END_NAMESPACE_TYPE => {}
            RES_XML_START_ELEMENT_TYPE => {
                let name_idx = u32_at(chunk, 20) as usize;
                let attr_start = u32_at(chunk, 24) as usize; // relative to chunk start? no: to the extended header
                let attr_size = u16_at(chunk, 26) as usize;
                let attr_count = u16_at(chunk, 28) as usize;
                let name = strings.get(name_idx).cloned().unwrap_or_default();
                let mut el = Element { name, ..Default::default() };
                // Attribute records start at hsize + attributeStart (attributeStart is relative to
                // the end of the ResXMLTree_node header, i.e. offset 16 within the chunk).
                let base = 16 + (u16_at(chunk, 24) as usize);
                let _ = attr_start;
                for i in 0..attr_count {
                    let a = base + i * attr_size.max(20);
                    if a + 20 > chunk.len() {
                        break;
                    }
                    let aname_idx = u32_at(chunk, a + 4) as usize;
                    let raw_idx = u32_at(chunk, a + 8);
                    let dtype = chunk[a + 15];
                    let dval = u32_at(chunk, a + 16);
                    let aname = strings.get(aname_idx).cloned().unwrap_or_default();
                    let res_id = res_map.get(aname_idx).copied().unwrap_or(0);
                    let value = match dtype {
                        TYPE_STRING => Value::Str(strings.get(dval as usize).cloned().unwrap_or_else(|| strings.get(raw_idx as usize).cloned().unwrap_or_default())),
                        TYPE_REFERENCE => Value::Reference(dval),
                        TYPE_INT_DEC | TYPE_INT_HEX => Value::Int(dval as i32),
                        TYPE_INT_BOOLEAN => Value::Bool(dval != 0),
                        other => Value::Other(other, dval),
                    };
                    el.attrs.push(Attribute { name: aname, res_id, value });
                }
                stack.push(el);
            }
            RES_XML_END_ELEMENT_TYPE => {
                if stack.len() > 1 {
                    let el = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(el);
                }
            }
            _ => {}
        }
        pos += size;
    }
    let mut root = stack.swap_remove(0);
    if root.children.len() == 1 {
        return Ok(root.children.remove(0));
    }
    Ok(root)
}
