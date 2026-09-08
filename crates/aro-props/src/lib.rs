//! Android system properties without Android's init.
//!
//! bionic's libc reads properties from `/dev/__properties__`, a directory that
//! init populates at boot:
//!
//! - `property_info`: a serialized trie mapping property names to a SELinux
//!   context (which selects the file holding the property) and a type.
//! - `properties_serial`: an (empty) property area whose serial counter bionic
//!   bumps on every write.
//! - one property area file per context, e.g. `u:object_r:default_prop:s0`.
//!
//! A property area is a 128 KiB mmap'd file: a 128-byte header followed by a
//! bump-allocated arena holding a trie keyed on the dot-separated name
//! segments. Each trie node keeps its siblings in a binary tree ordered by
//! (length, then bytes) of the segment. Layouts mirror
//! `bionic/libc/system_properties/include/system_properties/prop_area.h`
//! and `prop_info.h`.
//!
//! ARO uses a single context for every property; access control is done at
//! the process boundary, not by SELinux label.

use anyhow::{bail, Context, Result};
use std::path::Path;

pub const PA_SIZE: usize = 128 * 1024;
const HEADER_SIZE: usize = 128; // bytes_used, serial, magic, version, reserved[28]
const PROP_AREA_MAGIC: u32 = 0x504f_5250;
const PROP_AREA_VERSION: u32 = 0xfc6e_d0ab;
const PROP_VALUE_MAX: usize = 92;
const PROP_BT_SIZE: usize = 5 * 4; // namelen, prop, left, right, children
const PROP_INFO_SIZE: usize = 4 + PROP_VALUE_MAX; // serial + value union
const LONG_FLAG: u32 = 1 << 16;
const LONG_LEGACY_ERROR: &[u8] = b"Must use __system_property_read_callback() to read";

pub const DEFAULT_CONTEXT: &str = "u:object_r:default_prop:s0";

/// Builder for one property area file.
pub struct PropArea {
    data: Vec<u8>, // arena only (after the header)
}

impl Default for PropArea {
    fn default() -> Self {
        Self::new()
    }
}

impl PropArea {
    pub fn new() -> Self {
        let mut data = Vec::with_capacity(PA_SIZE - HEADER_SIZE);
        data.resize(PROP_BT_SIZE, 0); // root node: namelen 0, no links
        PropArea { data }
    }

    fn u32_at(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.data[off..off + 4].try_into().unwrap())
    }
    fn set_u32(&mut self, off: usize, v: u32) {
        self.data[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// Bump-allocate `size` bytes, 4-byte aligned. Returns the arena offset.
    fn alloc(&mut self, size: usize) -> Result<usize> {
        let aligned = (size + 3) & !3;
        let off = self.data.len();
        if off + aligned > PA_SIZE - HEADER_SIZE {
            bail!("property area full");
        }
        self.data.resize(off + aligned, 0);
        Ok(off)
    }

    fn new_bt(&mut self, name: &[u8]) -> Result<usize> {
        let off = self.alloc(PROP_BT_SIZE + name.len() + 1)?;
        self.set_u32(off, name.len() as u32);
        self.data[off + PROP_BT_SIZE..off + PROP_BT_SIZE + name.len()].copy_from_slice(name);
        Ok(off)
    }

    fn new_info(&mut self, name: &[u8], value: &[u8]) -> Result<usize> {
        let off = self.alloc(PROP_INFO_SIZE + name.len() + 1)?;
        self.data[off + PROP_INFO_SIZE..off + PROP_INFO_SIZE + name.len()].copy_from_slice(name);
        if value.len() >= PROP_VALUE_MAX {
            // Long property: value lives in its own allocation; the union holds
            // a legacy error string plus the offset relative to this prop_info.
            let long_off = self.alloc(value.len() + 1)?;
            self.data[long_off..long_off + value.len()].copy_from_slice(value);
            let serial = ((LONG_LEGACY_ERROR.len() as u32) << 24) | LONG_FLAG;
            self.set_u32(off, serial);
            self.data[off + 4..off + 4 + LONG_LEGACY_ERROR.len()].copy_from_slice(LONG_LEGACY_ERROR);
            self.set_u32(off + 4 + 56, (long_off - off) as u32);
        } else {
            self.set_u32(off, (value.len() as u32) << 24);
            self.data[off + 4..off + 4 + value.len()].copy_from_slice(value);
        }
        Ok(off)
    }

    fn cmp_name(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
        a.len().cmp(&b.len()).then_with(|| a.cmp(b))
    }

    fn bt_name(&self, bt: usize) -> &[u8] {
        let len = self.u32_at(bt) as usize;
        &self.data[bt + PROP_BT_SIZE..bt + PROP_BT_SIZE + len]
    }

    /// Find (or create) the sibling-tree node for `segment` starting at `bt`.
    fn find_bt(&mut self, mut bt: usize, segment: &[u8]) -> Result<usize> {
        loop {
            match Self::cmp_name(segment, self.bt_name(bt)) {
                std::cmp::Ordering::Equal => return Ok(bt),
                std::cmp::Ordering::Less => {
                    let left = self.u32_at(bt + 8) as usize;
                    if left != 0 {
                        bt = left;
                    } else {
                        let n = self.new_bt(segment)?;
                        self.set_u32(bt + 8, n as u32);
                        return Ok(n);
                    }
                }
                std::cmp::Ordering::Greater => {
                    let right = self.u32_at(bt + 12) as usize;
                    if right != 0 {
                        bt = right;
                    } else {
                        let n = self.new_bt(segment)?;
                        self.set_u32(bt + 12, n as u32);
                        return Ok(n);
                    }
                }
            }
        }
    }

    /// Add a property. Names are dot-separated; empty segments are invalid.
    pub fn add(&mut self, name: &str, value: &str) -> Result<()> {
        let mut current = 0usize; // root
        for segment in name.split('.') {
            if segment.is_empty() {
                bail!("invalid property name {name:?}");
            }
            let children = self.u32_at(current + 16) as usize;
            let subtree = if children != 0 {
                children
            } else {
                let n = self.new_bt(segment.as_bytes())?;
                self.set_u32(current + 16, n as u32);
                n
            };
            current = self.find_bt(subtree, segment.as_bytes())?;
        }
        if self.u32_at(current + 4) != 0 {
            bail!("duplicate property {name}");
        }
        let info = self.new_info(name.as_bytes(), value.as_bytes())?;
        self.set_u32(current + 4, info as u32);
        Ok(())
    }

    /// Serialize to the on-disk/mmap format (exactly PA_SIZE bytes).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; PA_SIZE];
        out[0..4].copy_from_slice(&(self.data.len() as u32).to_le_bytes()); // bytes_used
        out[8..12].copy_from_slice(&PROP_AREA_MAGIC.to_le_bytes());
        out[12..16].copy_from_slice(&PROP_AREA_VERSION.to_le_bytes());
        out[HEADER_SIZE..HEADER_SIZE + self.data.len()].copy_from_slice(&self.data);
        out
    }
}

/// Serialized property_info that maps every property to one context and type.
///
/// Layout follows `system/core/property_service/libpropertyinfoparser`:
/// header, then a contexts table (count + offsets), a types table, one
/// PropertyEntry, and a root TrieNodeInternal with no children.
pub fn property_info_single_context(context: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let push_u32 = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
    let align = |o: &mut Vec<u8>| while o.len() % 4 != 0 { o.push(0) };
    // header: current_version, minimum_supported_version, size, contexts_offset, types_offset, root_offset
    for _ in 0..6 {
        push_u32(&mut out, 0);
    }
    let contexts_offset = out.len();
    push_u32(&mut out, 1);
    let ctx_str_slot = out.len();
    push_u32(&mut out, 0);
    let ctx_str = out.len();
    out.extend_from_slice(context.as_bytes());
    out.push(0);
    align(&mut out);
    out[ctx_str_slot..ctx_str_slot + 4].copy_from_slice(&(ctx_str as u32).to_le_bytes());

    let types_offset = out.len();
    push_u32(&mut out, 1);
    let type_str_slot = out.len();
    push_u32(&mut out, 0);
    let type_str = out.len();
    out.extend_from_slice(b"string\0");
    align(&mut out);
    out[type_str_slot..type_str_slot + 4].copy_from_slice(&(type_str as u32).to_le_bytes());

    let empty_name = out.len();
    out.push(0);
    align(&mut out);

    let entry_offset = out.len();
    push_u32(&mut out, empty_name as u32); // name_offset
    push_u32(&mut out, 0); // namelen
    push_u32(&mut out, 0); // context_index
    push_u32(&mut out, 0); // type_index

    let root_offset = out.len();
    push_u32(&mut out, entry_offset as u32); // property_entry
    for _ in 0..6 {
        push_u32(&mut out, 0); // no children, prefixes, exact matches
    }

    let size = out.len() as u32;
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    out[4..8].copy_from_slice(&1u32.to_le_bytes());
    out[8..12].copy_from_slice(&size.to_le_bytes());
    out[12..16].copy_from_slice(&(contexts_offset as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(types_offset as u32).to_le_bytes());
    out[20..24].copy_from_slice(&(root_offset as u32).to_le_bytes());
    out
}

/// Parse `key=value` lines of an Android `build.prop`-style file.
pub fn parse_prop_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect()
}

/// Write a complete `/dev/__properties__` directory. Later entries override earlier ones.
pub fn write_properties_dir(dir: &Path, props: &[(String, String)]) -> Result<usize> {
    std::fs::create_dir_all(dir)?;
    let mut merged: Vec<(String, String)> = Vec::new();
    for (k, v) in props {
        if let Some(e) = merged.iter_mut().find(|(mk, _)| mk == k) {
            e.1 = v.clone();
        } else {
            merged.push((k.clone(), v.clone()));
        }
    }
    let mut area = PropArea::new();
    for (k, v) in &merged {
        area.add(k, v).with_context(|| format!("adding {k}"))?;
    }
    let write = |name: &str, bytes: &[u8]| -> Result<()> {
        let p = dir.join(name);
        std::fs::write(&p, bytes)?;
        // bionic refuses areas writable by group/other.
        std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
        Ok(())
    };
    write("property_info", &property_info_single_context(DEFAULT_CONTEXT))?;
    write("properties_serial", &PropArea::new().to_bytes())?;
    write(DEFAULT_CONTEXT, &area.to_bytes())?;
    Ok(merged.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_round_trip_layout() {
        let mut a = PropArea::new();
        a.add("ro.build.version.sdk", "37").unwrap();
        a.add("ro.build.id", "X").unwrap();
        a.add("ro.long", &"x".repeat(200)).unwrap();
        let b = a.to_bytes();
        assert_eq!(b.len(), PA_SIZE);
        assert_eq!(u32::from_le_bytes(b[8..12].try_into().unwrap()), PROP_AREA_MAGIC);
        // root's children offset points at "ro"
        let children = u32::from_le_bytes(b[HEADER_SIZE + 16..HEADER_SIZE + 20].try_into().unwrap()) as usize;
        assert_eq!(&b[HEADER_SIZE + children + PROP_BT_SIZE..HEADER_SIZE + children + PROP_BT_SIZE + 2], b"ro");
    }

    #[test]
    fn rejects_duplicates() {
        let mut a = PropArea::new();
        a.add("a.b", "1").unwrap();
        assert!(a.add("a.b", "2").is_err());
    }
}
