//! `EFI_DEVICE_PATH_PROTOCOL` parsing and text rendering (UEFI spec 10.3/10.6).
//!
//! Device paths are kept as raw bytes inside the structs that contain them so
//! they always round-trip; this module only interprets them for display.

use crate::Guid;

pub const TYPE_HARDWARE: u8 = 0x01;
pub const TYPE_ACPI: u8 = 0x02;
pub const TYPE_MESSAGING: u8 = 0x03;
pub const TYPE_MEDIA: u8 = 0x04;
pub const TYPE_BBS: u8 = 0x05;
pub const TYPE_END: u8 = 0x7f;
pub const END_INSTANCE: u8 = 0x01;
pub const END_ENTIRE: u8 = 0xff;

/// One node: generic header plus its type-specific payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub kind: u8,
    pub sub_type: u8,
    pub data: Vec<u8>,
}

/// A parsed device path: one or more instances, each a list of nodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicePath {
    pub instances: Vec<Vec<Node>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Truncated { offset: usize },
    BadLength { offset: usize, len: u16 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Truncated { offset } => write!(f, "device path truncated at offset {offset}"),
            Error::BadLength { offset, len } => write!(f, "device path node at offset {offset} has invalid length {len}"),
        }
    }
}

impl std::error::Error for Error {}

impl DevicePath {
    /// Parse a device path. Parsing stops at an End-Entire node (or the end of
    /// the buffer); an End-Instance node starts a new instance.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let mut instances = vec![Vec::new()];
        let mut offset = 0;
        while offset < bytes.len() {
            let header = bytes.get(offset..offset + 4).ok_or(Error::Truncated { offset })?;
            let len = u16::from_le_bytes([header[2], header[3]]);
            if (len as usize) < 4 {
                return Err(Error::BadLength { offset, len });
            }
            let body = bytes.get(offset + 4..offset + len as usize).ok_or(Error::Truncated { offset })?;
            offset += len as usize;
            match (header[0], header[1]) {
                (TYPE_END, END_ENTIRE) => break,
                (TYPE_END, END_INSTANCE) => instances.push(Vec::new()),
                (kind, sub_type) => {
                    instances.last_mut().unwrap().push(Node { kind, sub_type, data: body.to_vec() })
                }
            }
        }
        if instances.last().is_some_and(Vec::is_empty) && instances.len() > 1 {
            instances.pop();
        }
        Ok(Self { instances })
    }

    /// Text form, e.g. `PciRoot(0x0)/Pci(0x1,0x0)/HD(1,GPT,…)/\EFI\BOOT\BOOTX64.EFI`.
    pub fn to_text(&self) -> String {
        self.instances
            .iter()
            .map(|nodes| nodes.iter().map(Node::to_text).collect::<Vec<_>>().join("/"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl Node {
    pub fn type_name(&self) -> &'static str {
        match self.kind {
            TYPE_HARDWARE => "Hardware",
            TYPE_ACPI => "ACPI",
            TYPE_MESSAGING => "Messaging",
            TYPE_MEDIA => "Media",
            TYPE_BBS => "BBS",
            TYPE_END => "End",
            _ => "Unknown",
        }
    }

    pub fn to_text(&self) -> String {
        let r = Reader(&self.data);
        let text = match (self.kind, self.sub_type) {
            (TYPE_HARDWARE, 1) => r.u8(1).zip(r.u8(0)).map(|(dev, func)| format!("Pci(0x{dev:x},0x{func:x})")),
            (TYPE_HARDWARE, 2) => r.u8(0).map(|f| format!("PcCard(0x{f:x})")),
            (TYPE_HARDWARE, 3) => r.u32(0).zip(r.u64(4)).zip(r.u64(12))
                .map(|((t, s), e)| format!("MemoryMapped(0x{t:x},0x{s:x},0x{e:x})")),
            (TYPE_HARDWARE, 4) => vendor("VenHw", &r),
            (TYPE_HARDWARE, 5) => r.u32(0).map(|c| format!("Ctrl(0x{c:x})")),
            (TYPE_ACPI, 1) => r.u32(0).zip(r.u32(4)).map(|(hid, uid)| match hid {
                0x0a03_41d0 => format!("PciRoot(0x{uid:x})"),
                0x0a08_41d0 => format!("PcieRoot(0x{uid:x})"),
                0x0604_41d0 => format!("Floppy(0x{uid:x})"),
                _ => format!("Acpi({},0x{uid:x})", eisa_id(hid)),
            }),
            (TYPE_ACPI, 2) => r.u32(0).zip(r.u32(4)).zip(r.u32(8))
                .map(|((hid, uid), cid)| format!("AcpiEx({},{},0x{uid:x})", eisa_id(hid), eisa_id(cid))),
            (TYPE_ACPI, 3) => Some(format!("AcpiAdr({})", (0..self.data.len() / 4)
                .filter_map(|i| r.u32(i * 4)).map(|a| format!("0x{a:x}")).collect::<Vec<_>>().join(","))),
            (TYPE_MESSAGING, 1) => r.u8(0).zip(r.u8(1)).zip(r.u16(2)).map(|((p, s), l)| {
                format!("Ata({},{},{l})", if p == 0 { "Primary" } else { "Secondary" }, if s == 0 { "Master" } else { "Slave" })
            }),
            (TYPE_MESSAGING, 2) => r.u16(0).zip(r.u16(2)).map(|(pun, lun)| format!("Scsi(0x{pun:x},0x{lun:x})")),
            (TYPE_MESSAGING, 5) => r.u8(0).zip(r.u8(1)).map(|(port, intf)| format!("USB(0x{port:x},0x{intf:x})")),
            (TYPE_MESSAGING, 10) => vendor("VenMsg", &r),
            (TYPE_MESSAGING, 11) => r.u8(32).map(|if_type| {
                // Only the first 6 bytes are meaningful for Ethernet.
                let n = if if_type == 0 || if_type == 1 { 6 } else { 32 };
                format!("MAC({},0x{if_type:x})", crate::hex::to_hex(&self.data[..n]))
            }),
            (TYPE_MESSAGING, 12) => r.bytes(0, 4).zip(r.bytes(4, 4)).zip(r.u16(12)).zip(r.u8(14)).map(
                |(((local, remote), proto), is_static)| {
                    format!("IPv4({},{},{},{})", ipv4(remote), proto_name(proto),
                        if is_static == 1 { "Static" } else { "DHCP" }, ipv4(local))
                },
            ),
            (TYPE_MESSAGING, 13) => r.bytes(0, 16).zip(r.bytes(16, 16))
                .map(|(local, remote)| format!("IPv6({},{})", ipv6(remote), ipv6(local))),
            (TYPE_MESSAGING, 15) => r.u16(0).zip(r.u16(2)).zip(r.u8(4)).map(|((vid, pid), class)| {
                format!("UsbClass(0x{vid:x},0x{pid:x},0x{class:x},…)")
            }),
            (TYPE_MESSAGING, 18) => r.u16(0).zip(r.u16(2)).zip(r.u16(4))
                .map(|((hba, pm), lun)| format!("Sata(0x{hba:x},0x{pm:x},0x{lun:x})")),
            (TYPE_MESSAGING, 23) => r.u32(0).zip(r.bytes(4, 8)).map(|(nsid, eui)| {
                let eui = eui.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-");
                format!("NVMe(0x{nsid:x},{eui})")
            }),
            (TYPE_MESSAGING, 24) => Some(format!("Uri({})", String::from_utf8_lossy(&self.data))),
            (TYPE_MESSAGING, 30) => r.bytes(0, 6).map(|_| "Wi-Fi(…)".into()),
            (TYPE_MESSAGING, 31) => Some("SDCard".into()),
            (TYPE_MESSAGING, 22) => r.u16(0).map(|lun| format!("eMMC(0x{lun:x})")),
            (TYPE_MEDIA, 1) => hard_drive(&r),
            (TYPE_MEDIA, 2) => r.u32(0).zip(r.u64(4)).zip(r.u64(12))
                .map(|((entry, start), size)| format!("CDROM(0x{entry:x},0x{start:x},0x{size:x})")),
            (TYPE_MEDIA, 3) => vendor("VenMedia", &r),
            (TYPE_MEDIA, 4) => Some(utf16_string(&self.data)),
            (TYPE_MEDIA, 5) => r.guid(0).map(|g| format!("Media({g})")),
            (TYPE_MEDIA, 6) => r.guid(0).map(|g| format!("FvFile({g})")),
            (TYPE_MEDIA, 7) => r.guid(0).map(|g| format!("Fv({g})")),
            (TYPE_MEDIA, 8) => r.u64(4).zip(r.u64(12)).map(|(s, e)| format!("Offset(0x{s:x},0x{e:x})")),
            (TYPE_MEDIA, 9) => r.u64(0).zip(r.u64(8)).zip(r.guid(16))
                .map(|((s, e), g)| format!("RamDisk(0x{s:x},0x{e:x},{g})")),
            (TYPE_BBS, 1) => r.u16(0).zip(r.u16(2)).map(|(dev, flags)| {
                let desc = self.data.get(4..).unwrap_or_default();
                let desc = String::from_utf8_lossy(desc.split(|&b| b == 0).next().unwrap_or_default());
                format!("BBS(0x{dev:x},{desc},0x{flags:x})")
            }),
            _ => None,
        };
        text.unwrap_or_else(|| {
            format!("Path({},{},{})", self.kind, self.sub_type, crate::hex::to_hex(&self.data))
        })
    }
}

/// Bounds-checked little-endian reader over a node body.
struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&self, at: usize, n: usize) -> Option<&[u8]> {
        self.0.get(at..at.checked_add(n)?)
    }
    fn u8(&self, at: usize) -> Option<u8> {
        self.0.get(at).copied()
    }
    fn u16(&self, at: usize) -> Option<u16> {
        self.bytes(at, 2).map(|b| u16::from_le_bytes(b.try_into().unwrap()))
    }
    fn u32(&self, at: usize) -> Option<u32> {
        self.bytes(at, 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn u64(&self, at: usize) -> Option<u64> {
        self.bytes(at, 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }
    fn guid(&self, at: usize) -> Option<Guid> {
        self.bytes(at, 16).map(|b| Guid::from_bytes(b.try_into().unwrap()))
    }
}

fn vendor(prefix: &str, r: &Reader) -> Option<String> {
    let guid = r.guid(0)?;
    let rest = &r.0[16..];
    Some(if rest.is_empty() {
        format!("{prefix}({guid})")
    } else {
        format!("{prefix}({guid},{})", crate::hex::preview(rest, 16))
    })
}

fn hard_drive(r: &Reader) -> Option<String> {
    let part = r.u32(0)?;
    let start = r.u64(4)?;
    let size = r.u64(12)?;
    let sig = r.bytes(20, 16)?;
    let fmt = r.u8(36)?;
    let sig_type = r.u8(37)?;
    let fmt = match fmt {
        1 => "MBR",
        2 => "GPT",
        _ => "?",
    };
    let sig = match sig_type {
        1 => format!("0x{:08x}", u32::from_le_bytes(sig[..4].try_into().unwrap())),
        2 => Guid::from_bytes(sig.try_into().unwrap()).to_string(),
        _ => "0".into(),
    };
    Some(format!("HD({part},{fmt},{sig},0x{start:x},0x{size:x})"))
}

/// Compressed EISA PnP id, e.g. 0x0a0341d0 -> "PNP0A03".
fn eisa_id(id: u32) -> String {
    let lo = id & 0xffff;
    if lo & 0x8000 != 0 || lo == 0 {
        return format!("0x{id:x}");
    }
    let c = |shift: u32| (((lo >> shift) & 0x1f) as u8 + b'@') as char;
    format!("{}{}{}{:04X}", c(10), c(5), c(0), id >> 16)
}

fn ipv4(b: &[u8]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn ipv6(b: &[u8]) -> String {
    std::net::Ipv6Addr::from(<[u8; 16]>::try_from(b).unwrap()).to_string()
}

fn proto_name(p: u16) -> String {
    match p {
        6 => "TCP".into(),
        17 => "UDP".into(),
        _ => format!("0x{p:x}"),
    }
}

/// Decode a NUL-terminated (or unterminated) little-endian UTF-16 string.
pub fn utf16_string(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Render a whole device-path buffer, falling back to an error description.
pub fn describe(bytes: &[u8]) -> String {
    match DevicePath::parse(bytes) {
        Ok(path) => path.to_text(),
        Err(e) => format!("<{e}>"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn node(kind: u8, sub_type: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![kind, sub_type];
        v.extend_from_slice(&((body.len() + 4) as u16).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    pub fn utf16z(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    /// PciRoot(0)/Pci(0x1d,0)/HD(1,GPT,…)/\EFI\BOOT\BOOTX64.EFI
    pub fn sample_path() -> Vec<u8> {
        let mut hd = Vec::new();
        hd.extend_from_slice(&1u32.to_le_bytes());
        hd.extend_from_slice(&0x800u64.to_le_bytes());
        hd.extend_from_slice(&0x100000u64.to_le_bytes());
        hd.extend_from_slice(&crate::guid!("01234567-89ab-cdef-0123-456789abcdef").to_bytes());
        hd.extend_from_slice(&[2, 2]);
        [
            node(TYPE_ACPI, 1, &[0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0]),
            node(TYPE_HARDWARE, 1, &[0x00, 0x1d]),
            node(TYPE_MEDIA, 1, &hd),
            node(TYPE_MEDIA, 4, &utf16z("\\EFI\\BOOT\\BOOTX64.EFI")),
            node(TYPE_END, END_ENTIRE, &[]),
        ]
        .concat()
    }

    #[test]
    fn renders_common_path() {
        let path = DevicePath::parse(&sample_path()).unwrap();
        assert_eq!(
            path.to_text(),
            "PciRoot(0x0)/Pci(0x1d,0x0)/HD(1,GPT,01234567-89ab-cdef-0123-456789abcdef,0x800,0x100000)/\\EFI\\BOOT\\BOOTX64.EFI"
        );
    }

    #[test]
    fn multi_instance_and_unknown() {
        let bytes = [
            node(0x09, 0x09, &[0xaa]),
            node(TYPE_END, END_INSTANCE, &[]),
            node(TYPE_ACPI, 1, &[0xd0, 0x41, 0x01, 0x05, 1, 0, 0, 0]),
            node(TYPE_END, END_ENTIRE, &[]),
        ]
        .concat();
        assert_eq!(describe(&bytes), "Path(9,9,aa),Acpi(PNP0501,0x1)");
    }

    #[test]
    fn rejects_bad_length() {
        assert_eq!(DevicePath::parse(&[1, 1, 2, 0]), Err(Error::BadLength { offset: 0, len: 2 }));
        assert_eq!(DevicePath::parse(&[1, 1, 9, 0]), Err(Error::Truncated { offset: 0 }));
    }
}
