//! Wayland/XKB keyboard state translated to Android's virtual keyboard.
use crate::input_channel::{now_ns, KeyInput};
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CString},
    time::{Duration, Instant},
};

#[link(name = "xkbcommon")]
unsafe extern "C" {
    fn xkb_context_new(flags: u32) -> *mut c_void;
    fn xkb_context_unref(context: *mut c_void);
    fn xkb_keymap_new_from_string(
        context: *mut c_void,
        text: *const c_char,
        format: u32,
        flags: u32,
    ) -> *mut c_void;
    fn xkb_keymap_unref(keymap: *mut c_void);
    fn xkb_state_new(keymap: *mut c_void) -> *mut c_void;
    fn xkb_state_unref(state: *mut c_void);
    fn xkb_state_update_mask(
        state: *mut c_void,
        depressed: u32,
        latched: u32,
        locked: u32,
        depressed_layout: u32,
        latched_layout: u32,
        locked_layout: u32,
    ) -> u32;
    fn xkb_state_key_get_one_sym(state: *mut c_void, key: u32) -> u32;
    fn xkb_state_mod_name_is_active(state: *mut c_void, name: *const c_char, component: u32)
        -> i32;
    fn xkb_keymap_key_repeats(keymap: *mut c_void, key: u32) -> i32;
}

struct Xkb {
    context: *mut c_void,
    keymap: *mut c_void,
    state: *mut c_void,
}
impl Drop for Xkb {
    fn drop(&mut self) {
        unsafe {
            if !self.state.is_null() {
                xkb_state_unref(self.state);
            }
            if !self.keymap.is_null() {
                xkb_keymap_unref(self.keymap);
            }
            if !self.context.is_null() {
                xkb_context_unref(self.context);
            }
        }
    }
}
impl Xkb {
    fn parse(text: &[u8]) -> anyhow::Result<Self> {
        let text = CString::new(text.strip_suffix(&[0]).unwrap_or(text))?;
        let mut x = Self {
            context: std::ptr::null_mut(),
            keymap: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
        };
        unsafe {
            x.context = xkb_context_new(0);
            anyhow::ensure!(!x.context.is_null(), "xkb context allocation failed");
            x.keymap = xkb_keymap_new_from_string(x.context, text.as_ptr(), 1, 0);
            anyhow::ensure!(!x.keymap.is_null(), "invalid Wayland XKB keymap");
            x.state = xkb_state_new(x.keymap);
            anyhow::ensure!(!x.state.is_null(), "xkb state allocation failed");
        }
        Ok(x)
    }
    fn meta(&self) -> i32 {
        [
            (c"Shift", 1),
            (c"Mod1", 2),
            (c"Control", 0x1000),
            (c"Mod4", 0x10000),
        ]
        .into_iter()
        .fold(0, |meta, (name, bit)| {
            meta | if unsafe { xkb_state_mod_name_is_active(self.state, name.as_ptr(), 8) } > 0 {
                bit
            } else {
                0
            }
        })
    }
}

#[derive(Default)]
pub struct Keyboard {
    xkb: Option<Xkb>,
    pressed: BTreeMap<u32, KeyInput>,
    repeating: Option<(u32, Instant)>,
    rate: u32,
    delay: u32,
    focused: bool,
}

impl Keyboard {
    pub fn keymap(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.xkb = Some(Xkb::parse(bytes)?);
        Ok(())
    }
    pub fn modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        if let Some(x) = &self.xkb {
            unsafe {
                xkb_state_update_mask(x.state, depressed, latched, locked, 0, 0, group);
            }
        }
    }
    pub fn enter(&mut self) {
        self.focused = true;
    }
    pub fn leave(&mut self) -> Vec<KeyInput> {
        self.focused = false;
        self.repeating = None;
        std::mem::take(&mut self.pressed)
            .into_values()
            .map(|mut key| {
                key.action = 1;
                key.flags |= 0x20;
                key
            })
            .collect()
    }
    pub fn repeat_info(&mut self, rate: i32, delay: i32) {
        self.rate = rate.max(0) as u32;
        self.delay = delay.max(0) as u32;
        if self.rate == 0 {
            self.repeating = None;
        }
    }
    pub fn key(&mut self, scan: u32, down: bool) -> Option<KeyInput> {
        if !down {
            if self.repeating.as_ref().is_some_and(|(key, _)| *key == scan) {
                self.repeating = None;
            }
            let meta = self.xkb.as_ref().map(|x| x.meta());
            return self.pressed.remove(&scan).map(|mut key| {
                key.action = 1;
                key.repeat = 0;
                if let Some(meta) = meta {
                    key.meta = meta;
                }
                key
            });
        }
        if !self.focused {
            return None;
        }
        let (code, meta, repeats) = self.translate(scan)?;
        let key = KeyInput {
            code,
            scan: scan as i32,
            meta,
            action: 0,
            repeat: 0,
            down_time: now_ns(),
            flags: 0x8,
        };
        self.pressed.insert(scan, key);
        if repeats && self.rate > 0 {
            self.repeating = Some((
                scan,
                Instant::now() + Duration::from_millis(self.delay as u64),
            ));
        }
        Some(key)
    }
    fn translate(&self, scan: u32) -> Option<(i32, i32, bool)> {
        let x = self.xkb.as_ref()?;
        let sym = unsafe { xkb_state_key_get_one_sym(x.state, scan + 8) };
        let (code, shift) = symbol(sym)?;
        let mut meta = x.meta();
        if let Some(shift) = shift {
            meta = (meta & !1) | i32::from(shift);
        }
        Some((
            code,
            meta,
            unsafe { xkb_keymap_key_repeats(x.keymap, scan + 8) } != 0,
        ))
    }
    pub fn repeat(&mut self) -> Option<KeyInput> {
        let (scan, at) = self.repeating?;
        if Instant::now() < at {
            return None;
        }
        let (_, meta, _) = self.translate(scan)?;
        let key = self.pressed.get_mut(&scan)?;
        key.repeat = key.repeat.saturating_add(1);
        key.meta = meta;
        self.repeating = Some((
            scan,
            Instant::now() + Duration::from_nanos(1_000_000_000 / self.rate.max(1) as u64),
        ));
        Some(*key)
    }
}

/// Android keycode and the unshifted/shifted characters in our virtual KCM.
pub fn characters() -> Vec<(i32, char, char)> {
    let mut keys: Vec<_> = ('a'..='z')
        .enumerate()
        .map(|(i, c)| (29 + i as i32, c, c.to_ascii_uppercase()))
        .collect();
    keys.extend(
        "0123456789"
            .chars()
            .zip(")!@#$%^&*(".chars())
            .enumerate()
            .map(|(i, (a, b))| (7 + i as i32, a, b)),
    );
    keys.extend([
        (55, ',', '<'),
        (56, '.', '>'),
        (61, '\t', '\t'),
        (62, ' ', ' '),
        (66, '\n', '\n'),
        (68, '`', '~'),
        (69, '-', '_'),
        (70, '=', '+'),
        (71, '[', '{'),
        (72, ']', '}'),
        (73, '\\', '|'),
        (74, ';', ':'),
        (75, '\'', '"'),
        (76, '/', '?'),
    ]);
    keys.extend(('0'..='9').enumerate().map(|(i, c)| (144 + i as i32, c, c)));
    keys.extend([
        (154, '/', '/'),
        (155, '*', '*'),
        (156, '-', '-'),
        (157, '+', '+'),
        (158, '.', '.'),
        (159, ',', ','),
        (160, '\n', '\n'),
        (161, '=', '='),
    ]);
    keys
}

fn symbol(sym: u32) -> Option<(i32, Option<bool>)> {
    if let Some((code, base, shift)) = characters()
        .into_iter()
        .find(|(_, base, shift)| *base as u32 == sym || *shift as u32 == sym)
    {
        return Some((code, Some(base != shift && shift as u32 == sym)));
    }
    let code = match sym {
        0xff08 => 67,
        0xff09 | 0xfe20 => 61,
        0xff0d => 66,
        0xff8d => 160,
        0xff1b => 111,
        0xffff => 112,
        0xff50 => 122,
        0xff51 => 21,
        0xff52 => 19,
        0xff53 => 22,
        0xff54 => 20,
        0xff55 => 92,
        0xff56 => 93,
        0xff57 => 123,
        0xff63 => 124,
        0xffbe..=0xffc9 => 131 + (sym - 0xffbe) as i32,
        0xffe1 => 59,
        0xffe2 => 60,
        0xffe3 => 113,
        0xffe4 => 114,
        0xffe5 => 115,
        0xffe9 => 57,
        0xffea => 58,
        0xffeb => 117,
        0xffec => 118,
        0xffb0..=0xffb9 => 144 + (sym - 0xffb0) as i32,
        0xffaf => 154,
        0xffaa => 155,
        0xffad => 156,
        0xffab => 157,
        0xffae => 158,
        0xffac => 159,
        0xffbd => 161,
        0xff7f => 143,
        0xff95 => 122,
        0xff96 => 21,
        0xff97 => 19,
        0xff98 => 22,
        0xff99 => 20,
        0xff9a => 92,
        0xff9b => 93,
        0xff9c => 123,
        0xff9d => 23,
        0xff9e => 124,
        0xff9f => 112,
        _ => return None,
    };
    Some((code, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn printable_ascii_roundtrips_through_virtual_map() {
        for ch in ' '..='~' {
            let (code, shift) = symbol(ch as u32).unwrap();
            let (_, base, shifted) = characters()
                .into_iter()
                .find(|(k, _, _)| *k == code)
                .unwrap();
            assert_eq!(if shift.unwrap() { shifted } else { base }, ch);
        }
        assert_eq!(symbol(0xff51), Some((21, None)));
        assert_eq!(symbol(0xfe20), Some((61, None)));
        assert_eq!(symbol(0xffb5), Some((149, None)));
        assert!(characters().contains(&(149, '5', '5')));
    }
    #[test]
    fn focus_loss_releases_all_keys_and_cancels_repeat() {
        let mut keyboard = Keyboard::default();
        keyboard.pressed.insert(
            30,
            KeyInput {
                code: 29,
                down_time: 123,
                ..Default::default()
            },
        );
        keyboard.repeating = Some((30, Instant::now()));
        let released = keyboard.leave();
        assert_eq!(released.len(), 1);
        assert_eq!(
            (released[0].action, released[0].down_time, released[0].flags),
            (1, 123, 0x20)
        );
        assert!(keyboard.repeat().is_none());
        assert!(keyboard.key(30, false).is_none());
    }
}
