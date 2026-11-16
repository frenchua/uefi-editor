//! Capsule result variables (UEFI 2.11 §8.5.6): `Capsule####`, `CapsuleMax`
//! and `CapsuleLast` in the `EFI_CAPSULE_REPORT_GUID` namespace. Display only.

use crate::decode::Field;
use crate::{Guid, devpath, guids, hex};

/// `VariableTotalSize`, `Reserved`, `CapsuleGuid`, `CapsuleProcessed` (EFI_TIME),
/// `CapsuleStatus` (EFI_STATUS).
const HEADER_LEN: usize = 4 + 4 + 16 + 16 + 8;

/// Render an `EFI_TIME` (16 bytes).
pub fn efi_time(b: &[u8]) -> String {
    let year = u16::from_le_bytes([b[0], b[1]]);
    let nanos = u32::from_le_bytes(b[8..12].try_into().unwrap());
    let tz = i16::from_le_bytes([b[12], b[13]]);
    let mut s = format!("{year:04}-{:02}-{:02} {:02}:{:02}:{:02}", b[2], b[3], b[4], b[5], b[6]);
    if nanos != 0 {
        s.push_str(&format!(".{nanos:09}"));
    }
    // 0x07FF is EFI_UNSPECIFIED_TIMEZONE; otherwise minutes offset from UTC.
    if tz != 0x07ff {
        s.push_str(&format!(" UTC{:+03}:{:02}", tz / 60, (tz % 60).abs()));
    }
    s
}

/// `EFI_STATUS` name (UEFI Appendix D).
pub fn efi_status(v: u64) -> String {
    const ERRORS: &[&str] = &[
        "", "LOAD_ERROR", "INVALID_PARAMETER", "UNSUPPORTED", "BAD_BUFFER_SIZE", "BUFFER_TOO_SMALL", "NOT_READY",
        "DEVICE_ERROR", "WRITE_PROTECTED", "OUT_OF_RESOURCES", "VOLUME_CORRUPTED", "VOLUME_FULL", "NO_MEDIA",
        "MEDIA_CHANGED", "NOT_FOUND", "ACCESS_DENIED", "NO_RESPONSE", "NO_MAPPING", "TIMEOUT", "NOT_STARTED",
        "ALREADY_STARTED", "ABORTED", "ICMP_ERROR", "TFTP_ERROR", "PROTOCOL_ERROR", "INCOMPATIBLE_VERSION",
        "SECURITY_VIOLATION", "CRC_ERROR", "END_OF_MEDIA", "", "", "END_OF_FILE", "INVALID_LANGUAGE",
        "COMPROMISED_DATA", "IP_ADDRESS_CONFLICT", "HTTP_ERROR",
    ];
    let name = if v == 0 {
        "SUCCESS"
    } else if v >> 63 == 1 {
        ERRORS.get((v & !(1 << 63)) as usize).copied().unwrap_or("")
    } else {
        ""
    };
    if name.is_empty() { format!("0x{v:x}") } else { format!("EFI_{name} (0x{v:x})") }
}

pub fn result_fields(d: &[u8]) -> Result<Vec<Field>, String> {
    if d.len() < HEADER_LEN {
        return Err(format!("capsule result is {} bytes, shorter than its {HEADER_LEN}-byte header", d.len()));
    }
    let capsule = Guid::from_bytes(d[8..24].try_into().unwrap());
    let mut fields = vec![
        Field::new("Total size", format!("{} bytes", u32::from_le_bytes(d[0..4].try_into().unwrap()))),
        Field::new("Capsule GUID", capsule.to_string()),
        Field::new("Processed", efi_time(&d[24..40])),
        Field::new("Status", efi_status(u64::from_le_bytes(d[40..48].try_into().unwrap()))),
    ];
    let rest = &d[HEADER_LEN..];
    if capsule == guids::EFI_FIRMWARE_MANAGEMENT_CAPSULE_ID && rest.len() >= 20 {
        // EFI_CAPSULE_RESULT_VARIABLE_FMP: Version, PayloadIndex,
        // UpdateImageIndex, UpdateImageTypeId, then two NUL-terminated CHAR16 strings.
        let strings = &rest[20..];
        let file = devpath::utf16_string(strings);
        let after = (file.encode_utf16().count() + 1) * 2;
        let target = strings.get(after..).map(devpath::utf16_string).unwrap_or_default();
        fields.push(Field::new("FMP result", "").with(vec![
            Field::new("Version", u16::from_le_bytes([rest[0], rest[1]]).to_string()),
            Field::new("Payload index", rest[2].to_string()),
            Field::new("Update image index", rest[3].to_string()),
            Field::new("Update image type", Guid::from_bytes(rest[4..20].try_into().unwrap()).to_string()),
            Field::new("Capsule file name", file),
            Field::new("Capsule target", target),
        ]));
    } else if !rest.is_empty() {
        fields.push(Field::new("Result data", hex::preview(rest, 32)));
    }
    Ok(fields)
}

/// `CapsuleMax` / `CapsuleLast`: CHAR16[11] such as `Capsule0003`, no terminator.
pub fn name_fields(d: &[u8]) -> Result<Vec<Field>, String> {
    if d.len() != 22 {
        return Err(format!("expected 22 bytes (CHAR16[11]), found {}", d.len()));
    }
    Ok(vec![Field::new("Variable", devpath::utf16_string(d))])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fmp_result() {
        let mut d = Vec::new();
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&guids::EFI_FIRMWARE_MANAGEMENT_CAPSULE_ID.to_bytes());
        d.extend_from_slice(&2026u16.to_le_bytes());
        d.extend_from_slice(&[11, 16, 9, 30, 0, 0, 0, 0, 0, 0, 0xff, 0x07, 0, 0]);
        d.extend_from_slice(&((1u64 << 63) | 3).to_le_bytes());
        d.extend_from_slice(&[1, 0, 0, 1]);
        d.extend_from_slice(&[0xaa; 16]);
        for s in ["fw.cap", "bios"] {
            d.extend(s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        }
        let f = result_fields(&d).unwrap();
        assert_eq!(f[2].value, "2026-11-16 09:30:00");
        assert_eq!(f[3].value, "EFI_UNSUPPORTED (0x8000000000000003)");
        let fmp = &f[4].children;
        assert_eq!(fmp[4].value, "fw.cap");
        assert_eq!(fmp[5].value, "bios");
        assert_eq!(efi_status(0), "EFI_SUCCESS (0x0)");
    }

    #[test]
    fn decodes_capsule_name() {
        let d: Vec<u8> = "Capsule0003".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(name_fields(&d).unwrap()[0].value, "Capsule0003");
    }
}
