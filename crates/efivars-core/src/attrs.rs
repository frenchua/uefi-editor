//! `EFI_VARIABLE_*` attribute bits (UEFI spec 8.2).

pub const NON_VOLATILE: u32 = 0x0000_0001;
pub const BOOTSERVICE_ACCESS: u32 = 0x0000_0002;
pub const RUNTIME_ACCESS: u32 = 0x0000_0004;
pub const HARDWARE_ERROR_RECORD: u32 = 0x0000_0008;
pub const AUTHENTICATED_WRITE_ACCESS: u32 = 0x0000_0010;
pub const TIME_BASED_AUTHENTICATED_WRITE_ACCESS: u32 = 0x0000_0020;
pub const APPEND_WRITE: u32 = 0x0000_0040;
pub const ENHANCED_AUTHENTICATED_ACCESS: u32 = 0x0000_0080;

/// Attributes whose writes must carry a signed authentication descriptor.
pub const AUTH_MASK: u32 =
    AUTHENTICATED_WRITE_ACCESS | TIME_BASED_AUTHENTICATED_WRITE_ACCESS | ENHANCED_AUTHENTICATED_ACCESS;

const NAMES: [(u32, &str); 8] = [
    (NON_VOLATILE, "NV"),
    (BOOTSERVICE_ACCESS, "BS"),
    (RUNTIME_ACCESS, "RT"),
    (HARDWARE_ERROR_RECORD, "HR"),
    (AUTHENTICATED_WRITE_ACCESS, "AW"),
    (TIME_BASED_AUTHENTICATED_WRITE_ACCESS, "AT"),
    (APPEND_WRITE, "AP"),
    (ENHANCED_AUTHENTICATED_ACCESS, "EA"),
];

/// Short form such as `NV+BS+RT`, plus any unknown bits in hex.
pub fn describe(attributes: u32) -> String {
    let mut parts: Vec<String> = NAMES
        .iter()
        .filter(|(bit, _)| attributes & bit != 0)
        .map(|(_, name)| (*name).to_string())
        .collect();
    let known = NAMES.iter().fold(0, |acc, (bit, _)| acc | bit);
    if attributes & !known != 0 {
        parts.push(format!("0x{:x}", attributes & !known));
    }
    if parts.is_empty() { "none".into() } else { parts.join("+") }
}

pub fn is_authenticated(attributes: u32) -> bool {
    attributes & AUTH_MASK != 0
}
