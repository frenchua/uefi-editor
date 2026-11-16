//! `EFI_LOAD_OPTION` (UEFI spec 3.1.3): the payload of `Boot####`,
//! `Driver####`, `SysPrep####` and `PlatformRecovery####`.

use std::io::Cursor;

use binrw::helpers::until_eof;
use binrw::{BinRead, BinWrite, NullWideString, binrw};

use crate::devpath;

pub const LOAD_OPTION_ACTIVE: u32 = 0x0000_0001;
pub const LOAD_OPTION_FORCE_RECONNECT: u32 = 0x0000_0002;
pub const LOAD_OPTION_HIDDEN: u32 = 0x0000_0008;
pub const LOAD_OPTION_CATEGORY_MASK: u32 = 0x0000_1f00;
pub const LOAD_OPTION_CATEGORY_APP: u32 = 0x0000_0100;

#[binrw]
#[brw(little)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadOption {
    pub attributes: u32,
    #[br(temp)]
    #[bw(try_calc = u16::try_from(file_path_list.len()))]
    file_path_list_length: u16,
    pub description: NullWideString,
    /// Raw device path list; interpret with [`devpath::DevicePath::parse`].
    #[br(count = file_path_list_length)]
    pub file_path_list: Vec<u8>,
    #[br(parse_with = until_eof)]
    pub optional_data: Vec<u8>,
}

impl LoadOption {
    pub fn parse(bytes: &[u8]) -> binrw::BinResult<Self> {
        Self::read(&mut Cursor::new(bytes))
    }

    pub fn to_bytes(&self) -> binrw::BinResult<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        self.write(&mut out)?;
        Ok(out.into_inner())
    }

    pub fn is_active(&self) -> bool {
        self.attributes & LOAD_OPTION_ACTIVE != 0
    }

    pub fn device_path_text(&self) -> String {
        devpath::describe(&self.file_path_list)
    }

    pub fn attribute_names(&self) -> String {
        let mut names = Vec::new();
        if self.attributes & LOAD_OPTION_ACTIVE != 0 {
            names.push("ACTIVE");
        }
        if self.attributes & LOAD_OPTION_FORCE_RECONNECT != 0 {
            names.push("FORCE_RECONNECT");
        }
        if self.attributes & LOAD_OPTION_HIDDEN != 0 {
            names.push("HIDDEN");
        }
        names.push(if self.attributes & LOAD_OPTION_CATEGORY_MASK == LOAD_OPTION_CATEGORY_APP {
            "CATEGORY_APP"
        } else {
            "CATEGORY_BOOT"
        });
        names.join(" | ")
    }
}

/// `Boot####`-style name check: prefix followed by exactly four hex digits.
pub fn is_numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Decode a `BootOrder`-style array of u16 option numbers.
pub fn parse_order(bytes: &[u8]) -> Result<Vec<u16>, String> {
    if bytes.len() % 2 != 0 {
        return Err(format!("odd length {} for u16 array", bytes.len()));
    }
    Ok(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect())
}

pub fn encode_order(order: &[u16]) -> Vec<u8> {
    order.iter().flat_map(|n| n.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devpath::tests::{sample_path, utf16z};

    fn sample() -> Vec<u8> {
        let path = sample_path();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&LOAD_OPTION_ACTIVE.to_le_bytes());
        bytes.extend_from_slice(&(path.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&utf16z("Linux Boot Manager"));
        bytes.extend_from_slice(&path);
        bytes.extend_from_slice(b"WINDOWS\0");
        bytes
    }

    #[test]
    fn round_trip() {
        let bytes = sample();
        let opt = LoadOption::parse(&bytes).unwrap();
        assert!(opt.is_active());
        assert_eq!(opt.description.to_string(), "Linux Boot Manager");
        assert!(opt.device_path_text().ends_with("\\EFI\\BOOT\\BOOTX64.EFI"));
        assert_eq!(opt.optional_data, b"WINDOWS\0");
        assert_eq!(opt.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn edit_recomputes_length() {
        let mut opt = LoadOption::parse(&sample()).unwrap();
        opt.attributes &= !LOAD_OPTION_ACTIVE;
        opt.file_path_list.truncate(4);
        let reparsed = LoadOption::parse(&opt.to_bytes().unwrap()).unwrap();
        assert_eq!(reparsed, opt);
    }

    #[test]
    fn truncated_is_error() {
        let bytes = sample();
        assert!(LoadOption::parse(&bytes[..10]).is_err());
    }

    #[test]
    fn order_helpers() {
        assert_eq!(parse_order(&[1, 0, 0x10, 0]).unwrap(), vec![1, 0x10]);
        assert_eq!(encode_order(&[1, 0x10]), vec![1, 0, 0x10, 0]);
        assert!(parse_order(&[1]).is_err());
        assert!(is_numbered("Boot00A1", "Boot"));
        assert!(!is_numbered("BootOrder", "Boot"));
    }
}
