//! Common Platform Error Record (UEFI 2.11 Appendix N), the payload of
//! `HwErrRec####` variables. Decoded for display only.

use crate::decode::Field;
use crate::{Guid, guids, hex};

const HEADER_LEN: usize = 128;
const SECTION_DESCRIPTOR_LEN: usize = 72;

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(d[at..at + 2].try_into().unwrap())
}
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}
fn u64_at(d: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(d[at..at + 8].try_into().unwrap())
}
fn guid_at(d: &[u8], at: usize) -> Guid {
    Guid::from_bytes(d[at..at + 16].try_into().unwrap())
}

fn severity(v: u32) -> String {
    match v {
        0 => "Recoverable (0)".into(),
        1 => "Fatal (1)".into(),
        2 => "Corrected (2)".into(),
        3 => "Informational (3)".into(),
        n => format!("reserved ({n})"),
    }
}

/// BCD revision: byte 0 minor, byte 1 major.
fn revision(v: u16) -> String {
    format!("{:x}.{:02x}", v >> 8, v & 0xff)
}

/// Timestamp: seconds, minutes, hours, flags, day, month, year, century (BCD).
fn timestamp(b: &[u8]) -> String {
    let precise = if b[3] & 1 != 0 { " (precise)" } else { "" };
    format!("{:02x}{:02x}-{:02x}-{:02x} {:02x}:{:02x}:{:02x}{precise}", b[7], b[6], b[5], b[4], b[2], b[1], b[0])
}

fn flag_names(v: u32, names: &[(u32, &str)]) -> String {
    let set: Vec<&str> = names.iter().filter(|(bit, _)| v & bit != 0).map(|(_, n)| *n).collect();
    if set.is_empty() { format!("0x{v:x}") } else { format!("0x{v:x} {}", set.join(" | ")) }
}

fn section_type_name(g: &Guid) -> Option<&'static str> {
    Some(match *g {
        guids::CPER_PROCESSOR_GENERIC => "Processor Generic",
        guids::CPER_PLATFORM_MEMORY => "Platform Memory",
        guids::CPER_PCIE => "PCI Express",
        _ => return None,
    })
}

pub fn fields(d: &[u8]) -> Result<Vec<Field>, String> {
    if d.len() < HEADER_LEN {
        return Err(format!("CPER record is {} bytes, shorter than its 128-byte header", d.len()));
    }
    if &d[0..4] != b"CPER" || u32_at(d, 6) != 0xffff_ffff {
        return Err("missing CPER signature".into());
    }
    let count = u16_at(d, 10) as usize;
    let valid = u32_at(d, 16);
    let mut fields = vec![
        Field::new("Revision", revision(u16_at(d, 4))),
        Field::new("Error severity", severity(u32_at(d, 12))),
        Field::new("Record length", format!("{} bytes", u32_at(d, 20))),
        Field::new("Timestamp", if valid & 2 != 0 { timestamp(&d[24..32]) } else { "not valid".into() }),
        Field::new("Platform ID", if valid & 1 != 0 { guid_at(d, 32).to_string() } else { "not valid".into() }),
        Field::new("Partition ID", if valid & 4 != 0 { guid_at(d, 48).to_string() } else { "not valid".into() }),
        Field::new("Creator ID", guid_at(d, 64).to_string()),
        Field::new("Notification type", guid_at(d, 80).to_string()),
        Field::new("Record ID", format!("0x{:016x}", u64_at(d, 96))),
        Field::new("Flags", flag_names(u32_at(d, 104), &[(1, "RECOVERED"), (2, "PREVERR"), (4, "SIMULATED")])),
        Field::new("Persistence info", format!("0x{:016x}", u64_at(d, 108))),
    ];
    let mut sections = Vec::new();
    for i in 0..count {
        let at = HEADER_LEN + i * SECTION_DESCRIPTOR_LEN;
        let Some(s) = d.get(at..at + SECTION_DESCRIPTOR_LEN) else {
            sections.push(Field::new(format!("[{i}]"), "descriptor truncated"));
            break;
        };
        let ty = guid_at(s, 16);
        let ty_text = match section_type_name(&ty) {
            Some(n) => format!("{n} ({ty})"),
            None => ty.to_string(),
        };
        let fru_text: String = s[52..72].iter().take_while(|&&b| b != 0).map(|&b| hex::printable(b)).collect();
        let (offset, len) = (u32_at(s, 0) as usize, u32_at(s, 4) as usize);
        let body = d.get(offset..offset.saturating_add(len)).map_or("out of bounds".into(), |b| hex::preview(b, 32));
        sections.push(Field::new(format!("[{i}]"), ty_text).with(vec![
            Field::new("Severity", severity(u32_at(s, 48))),
            Field::new("Revision", revision(u16_at(s, 8))),
            Field::new(
                "Flags",
                flag_names(u32_at(s, 12), &[
                    (1, "PRIMARY"), (2, "CONTAINMENT_WARNING"), (4, "RESET"), (8, "THRESHOLD_EXCEEDED"),
                    (16, "RESOURCE_NOT_ACCESSIBLE"), (32, "LATENT_ERROR"), (64, "PROPAGATED"), (128, "OVERFLOW"),
                ]),
            ),
            Field::new("FRU ID", if s[10] & 1 != 0 { guid_at(s, 32).to_string() } else { "not valid".into() }),
            Field::new("FRU text", if s[10] & 2 != 0 { fru_text } else { "not valid".into() }),
            Field::new("Data", format!("{len} bytes at offset {offset}: {body}")),
        ]));
    }
    fields.push(Field::new("Sections", count.to_string()).with(sections));
    Ok(fields)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Vec<u8> {
        let mut d = vec![0u8; HEADER_LEN + SECTION_DESCRIPTOR_LEN + 4];
        d[0..4].copy_from_slice(b"CPER");
        d[4..6].copy_from_slice(&0x0101u16.to_le_bytes());
        d[6..10].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
        d[10..12].copy_from_slice(&1u16.to_le_bytes());
        d[12..16].copy_from_slice(&2u32.to_le_bytes());
        d[16..20].copy_from_slice(&2u32.to_le_bytes());
        let len = d.len() as u32;
        d[20..24].copy_from_slice(&len.to_le_bytes());
        d[24..32].copy_from_slice(&[0x56, 0x34, 0x12, 1, 0x16, 0x11, 0x26, 0x20]);
        let s = HEADER_LEN;
        d[s..s + 4].copy_from_slice(&((HEADER_LEN + SECTION_DESCRIPTOR_LEN) as u32).to_le_bytes());
        d[s + 4..s + 8].copy_from_slice(&4u32.to_le_bytes());
        d[s + 16..s + 32].copy_from_slice(&guids::CPER_PCIE.to_bytes());
        d
    }

    #[test]
    fn decodes_header_and_sections() {
        let f = fields(&sample()).unwrap();
        let get = |label: &str| f.iter().find(|x| x.label == label).unwrap().value.clone();
        assert_eq!(get("Revision"), "1.01");
        assert_eq!(get("Error severity"), "Corrected (2)");
        assert_eq!(get("Timestamp"), "2026-11-16 12:34:56 (precise)");
        assert_eq!(get("Platform ID"), "not valid");
        let sections = &f.last().unwrap().children;
        assert!(sections[0].value.starts_with("PCI Express"));
        assert!(fields(&sample()[..100]).is_err());
    }
}
