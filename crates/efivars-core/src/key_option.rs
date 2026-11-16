//! `EFI_KEY_OPTION` (UEFI spec 3.1.6): the payload of `Key####` hot-key variables.

use std::io::Cursor;

use binrw::{BinRead, BinWrite, binrw};

#[binrw]
#[brw(little)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyOption {
    /// Packed `EFI_BOOT_KEY_DATA`; bits 30-31 hold the key count.
    pub key_data: u32,
    pub boot_option_crc: u32,
    pub boot_option: u16,
    #[br(count = key_data >> 30)]
    pub keys: Vec<InputKey>,
}

#[binrw]
#[brw(little)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: u16,
}

impl KeyOption {
    pub fn parse(bytes: &[u8]) -> binrw::BinResult<Self> {
        Self::read(&mut Cursor::new(bytes))
    }

    pub fn to_bytes(&self) -> binrw::BinResult<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        self.write(&mut out)?;
        Ok(out.into_inner())
    }

    pub fn modifiers(&self) -> String {
        const MODS: [(u32, &str); 6] =
            [(8, "Shift"), (9, "Ctrl"), (10, "Alt"), (11, "Logo"), (12, "Menu"), (13, "SysReq")];
        let names: Vec<&str> =
            MODS.iter().filter(|(bit, _)| self.key_data & (1 << bit) != 0).map(|(_, n)| *n).collect();
        if names.is_empty() { "none".into() } else { names.join("+") }
    }
}

/// Modifier bits of `EFI_BOOT_KEY_DATA` (bits 8-13).
pub const MODIFIER_MASK: u32 = 0x0000_3f00;
/// `InputKeyCount` (bits 30-31).
pub const KEY_COUNT_MASK: u32 = 0xc000_0000;
pub const MAX_KEYS: usize = 3;

/// Named `EFI_INPUT_KEY.ScanCode` values (UEFI spec Table 12.x).
pub const SCAN_CODES: &[(u16, &str)] = &[
    (0x01, "Up"), (0x02, "Down"), (0x03, "Right"), (0x04, "Left"), (0x05, "Home"), (0x06, "End"),
    (0x07, "Insert"), (0x08, "Delete"), (0x09, "PageUp"), (0x0a, "PageDown"),
    (0x0b, "F1"), (0x0c, "F2"), (0x0d, "F3"), (0x0e, "F4"), (0x0f, "F5"), (0x10, "F6"), (0x11, "F7"),
    (0x12, "F8"), (0x13, "F9"), (0x14, "F10"), (0x15, "F11"), (0x16, "F12"), (0x17, "Esc"),
    (0x48, "Pause"),
];

impl InputKey {
    /// `F12`, `'p'`, or `scan=0x..,char=0x..` for anything else.
    pub fn to_text(self) -> String {
        if self.unicode_char == 0
            && let Some((_, name)) = SCAN_CODES.iter().find(|(c, _)| *c == self.scan_code)
        {
            return (*name).to_string();
        }
        if self.scan_code == 0
            && let Some(c) = char::from_u32(self.unicode_char.into()).filter(|c| c.is_ascii_graphic())
        {
            return format!("'{c}'");
        }
        format!("scan=0x{:x},char=0x{:x}", self.scan_code, self.unicode_char)
    }

    pub fn from_text(text: &str) -> Result<Self, String> {
        let t = text.trim();
        if let Some((code, _)) = SCAN_CODES.iter().find(|(_, n)| n.eq_ignore_ascii_case(t)) {
            return Ok(Self { scan_code: *code, unicode_char: 0 });
        }
        let quoted = t.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(t);
        let mut chars = quoted.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && let Ok(unit) = u16::try_from(c as u32)
        {
            return Ok(Self { scan_code: 0, unicode_char: unit });
        }
        let mut key = Self { scan_code: 0, unicode_char: 0 };
        for part in t.split(',') {
            let (name, value) = part.split_once('=').ok_or_else(|| format!("{t:?} is not a key; use e.g. F12, 'p' or scan=0x17,char=0x0"))?;
            let value = value.trim();
            let v = match value.strip_prefix("0x") {
                Some(h) => u16::from_str_radix(h, 16),
                None => value.parse(),
            }
            .map_err(|_| format!("{value:?} is not a 16-bit number"))?;
            match name.trim() {
                "scan" => key.scan_code = v,
                "char" => key.unicode_char = v,
                other => return Err(format!("unknown key part {other:?}; use scan= and char=")),
            }
        }
        Ok(key)
    }
}

/// CRC-32 as computed by `EFI_BOOT_SERVICES.CalculateCrc32` (IEEE 802.3),
/// used for `EFI_KEY_OPTION.BootOptionCrc`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn key_text_round_trips() {
        for text in ["F12", "Esc", "'p'", "scan=0x0,char=0x0"] {
            assert_eq!(InputKey::from_text(text).unwrap().to_text(), text);
        }
        assert_eq!(InputKey::from_text("f1").unwrap().scan_code, 0x0b);
        assert_eq!(InputKey::from_text("p").unwrap().unicode_char, u16::from(b'p'));
        assert!(InputKey::from_text("nonsense").is_err());
    }

    #[test]
    fn round_trip() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((1u32 << 30) | (1 << 9)).to_le_bytes());
        bytes.extend_from_slice(&0xdead_beefu32.to_le_bytes());
        bytes.extend_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&[0x0b, 0, 0, 0]);
        let k = KeyOption::parse(&bytes).unwrap();
        assert_eq!(k.keys.len(), 1);
        assert_eq!(k.modifiers(), "Ctrl");
        assert_eq!(k.to_bytes().unwrap(), bytes);
    }
}
