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

#[cfg(test)]
mod tests {
    use super::*;

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
