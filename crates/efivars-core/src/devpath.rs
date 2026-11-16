//! `EFI_DEVICE_PATH_PROTOCOL` parsing and text rendering (UEFI spec 10.3/10.6).
//!
//! Device paths are kept as raw bytes inside the structs that contain them so
//! they always round-trip. This module renders them as text, parses the text
//! form back ([`Node::from_text`]) and re-serializes edited paths
//! ([`DevicePath::to_bytes`]).
//!
//! [`Node::to_text`] is a friendly, possibly abbreviated rendering for display.
//! [`Node::edit_text`] is guaranteed to parse back to the identical node: it is
//! the friendly form when that round-trips, else the generic
//! `Path(type,subtype,hex)` form.

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

    /// Serialize: nodes, an End-Instance node between instances, and a final
    /// End-Entire node. A single empty instance encodes as just End-Entire.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let only_empty = self.instances.len() == 1 && self.instances[0].is_empty();
        if !only_empty && self.instances.iter().any(Vec::is_empty) {
            return Err("device path has an empty instance".into());
        }
        let mut out = Vec::new();
        for (i, nodes) in self.instances.iter().enumerate() {
            for node in nodes {
                if node.kind == TYPE_END {
                    return Err("End nodes are implicit; use instance separators instead".into());
                }
                let len = u16::try_from(node.data.len() + 4)
                    .map_err(|_| format!("{} node is too large ({} bytes)", node.type_name(), node.data.len()))?;
                out.extend_from_slice(&[node.kind, node.sub_type]);
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&node.data);
            }
            if i + 1 < self.instances.len() {
                out.extend_from_slice(&[TYPE_END, END_INSTANCE, 4, 0]);
            }
        }
        out.extend_from_slice(&[TYPE_END, END_ENTIRE, 4, 0]);
        Ok(out)
    }

    /// Split a packed array of device paths, each terminated by End-Entire
    /// (as in `EFI_LOAD_OPTION.FilePathList`), requiring every path to
    /// re-serialize exactly.
    pub fn parse_list(bytes: &[u8]) -> Result<Vec<Self>, String> {
        let mut paths = Vec::new();
        let (mut start, mut offset) = (0, 0);
        while offset < bytes.len() {
            let header = bytes.get(offset..offset + 4).ok_or(Error::Truncated { offset }.to_string())?;
            let len = u16::from_le_bytes([header[2], header[3]]) as usize;
            if len < 4 || offset + len > bytes.len() {
                return Err(Error::BadLength { offset, len: len as u16 }.to_string());
            }
            offset += len;
            if header[0] == TYPE_END && header[1] == END_ENTIRE {
                paths.push(Self::parse_exact(&bytes[start..offset])?);
                start = offset;
            }
        }
        if start != bytes.len() {
            return Err("device path list does not end with an End node".into());
        }
        Ok(paths)
    }

    pub fn list_to_bytes(paths: &[Self]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        for p in paths {
            out.extend(p.to_bytes()?);
        }
        Ok(out)
    }

    /// Parse `bytes` and confirm that re-serializing reproduces them exactly
    /// (no trailing data after End-Entire, no non-canonical End nodes). Only
    /// such paths are safe to edit structurally.
    pub fn parse_exact(bytes: &[u8]) -> Result<Self, String> {
        let path = Self::parse(bytes).map_err(|e| e.to_string())?;
        if path.to_bytes()? != bytes {
            return Err("device path has trailing or non-canonical data".into());
        }
        Ok(path)
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

    /// Friendly text for display; long vendor data is abbreviated.
    pub fn to_text(&self) -> String {
        self.render(false)
    }

    /// Text that [`Node::from_text`] parses back to exactly this node.
    pub fn edit_text(&self) -> String {
        let text = self.render(true);
        match Node::from_text(&text) {
            Ok(node) if node == *self => text,
            _ => self.generic_text(),
        }
    }

    /// Lossless `Path(type,subtype,hex)` form, valid for any node.
    pub fn generic_text(&self) -> String {
        format!("Path({},{},{})", self.kind, self.sub_type, crate::hex::to_hex(&self.data))
    }

    fn render(&self, full: bool) -> String {
        let r = Reader(&self.data);
        let vendor = |prefix: &str| vendor(prefix, &r, full);
        let text = match (self.kind, self.sub_type) {
            (TYPE_HARDWARE, 1) => r.u8(1).zip(r.u8(0)).map(|(dev, func)| format!("Pci(0x{dev:x},0x{func:x})")),
            (TYPE_HARDWARE, 2) => r.u8(0).map(|f| format!("PcCard(0x{f:x})")),
            (TYPE_HARDWARE, 3) => r.u32(0).zip(r.u64(4)).zip(r.u64(12))
                .map(|((t, s), e)| format!("MemoryMapped(0x{t:x},0x{s:x},0x{e:x})")),
            (TYPE_HARDWARE, 4) => vendor("VenHw"),
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
            (TYPE_MESSAGING, 10) => vendor("VenMsg"),
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
            (TYPE_MESSAGING, 17) => r.u8(0).map(|lun| format!("Unit(0x{lun:x})")),
            (TYPE_MESSAGING, 18) => r.u16(0).zip(r.u16(2)).zip(r.u16(4))
                .map(|((hba, pm), lun)| format!("Sata(0x{hba:x},0x{pm:x},0x{lun:x})")),
            (TYPE_MESSAGING, 20) => r.u16(0).map(|vlan| format!("Vlan({vlan})")),
            (TYPE_MESSAGING, 23) => r.u32(0).zip(r.bytes(4, 8)).map(|(nsid, eui)| {
                let eui = eui.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-");
                format!("NVMe(0x{nsid:x},{eui})")
            }),
            (TYPE_MESSAGING, 24) => Some(format!("Uri({})", String::from_utf8_lossy(&self.data))),
            (TYPE_MESSAGING, 26) => r.u8(0).map(|slot| format!("SD(0x{slot:x})")),
            (TYPE_MESSAGING, 28) => r.bytes(0, 32).map(|ssid| {
                let ssid = ssid.split(|&b| b == 0).next().unwrap_or_default();
                format!("Wi-Fi({})", String::from_utf8_lossy(ssid))
            }),
            (TYPE_MESSAGING, 29) => r.u8(0).map(|slot| format!("eMMC(0x{slot:x})")),
            (TYPE_MEDIA, 1) => hard_drive(&r),
            (TYPE_MEDIA, 2) => r.u32(0).zip(r.u64(4)).zip(r.u64(12))
                .map(|((entry, start), size)| format!("CDROM(0x{entry:x},0x{start:x},0x{size:x})")),
            (TYPE_MEDIA, 3) => vendor("VenMedia"),
            (TYPE_MEDIA, 4) => Some(utf16_string(&self.data)),
            (TYPE_MEDIA, 5) => r.guid(0).map(|g| format!("Media({g})")),
            (TYPE_MEDIA, 6) => r.guid(0).map(|g| format!("FvFile({g})")),
            (TYPE_MEDIA, 7) => r.guid(0).map(|g| format!("Fv({g})")),
            (TYPE_MEDIA, 8) => r.u64(4).zip(r.u64(12)).map(|(s, e)| format!("Offset(0x{s:x},0x{e:x})")),
            (TYPE_MEDIA, 9) => r.u64(0).zip(r.u64(8)).zip(r.guid(16)).zip(r.u16(32))
                .map(|(((s, e), g), inst)| format!("RamDisk(0x{s:x},0x{e:x},{inst},{g})")),
            (TYPE_BBS, 1) => r.u16(0).zip(r.u16(2)).map(|(dev, flags)| {
                let desc = self.data.get(4..).unwrap_or_default();
                let desc = String::from_utf8_lossy(desc.split(|&b| b == 0).next().unwrap_or_default());
                format!("BBS(0x{dev:x},{desc},0x{flags:x})")
            }),
            _ => None,
        };
        text.unwrap_or_else(|| self.generic_text())
    }

    /// Parse one node from its text form (UEFI spec 10.6.1). Text that is not
    /// of the form `Name(args)` is taken as a FilePath node, e.g.
    /// `\EFI\BOOT\BOOTX64.EFI`.
    pub fn from_text(text: &str) -> Result<Node, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("empty node".into());
        }
        let Some((name, inner)) = split_call(text)? else {
            return Ok(Node { kind: TYPE_MEDIA, sub_type: 4, data: utf16z(text) });
        };
        let args: Vec<&str> = if inner.trim().is_empty() { Vec::new() } else { inner.split(',').map(str::trim).collect() };
        let a = Args { name, args: &args };
        let mut w = Vec::new();
        let (kind, sub_type) = match name {
            "Path" => {
                a.count(3)?;
                let kind: u8 = a.num(0)?;
                if kind == TYPE_END {
                    return Err("End nodes are implicit; use instance separators instead".into());
                }
                w.extend(a.hex(2)?);
                (kind, a.num(1)?)
            }
            "Pci" => {
                a.count(2)?;
                w.extend([a.num::<u8>(1)?, a.num::<u8>(0)?]);
                (TYPE_HARDWARE, 1)
            }
            "PcCard" => {
                a.count(1)?;
                w.push(a.num::<u8>(0)?);
                (TYPE_HARDWARE, 2)
            }
            "MemoryMapped" => {
                a.count(3)?;
                put(&mut w, a.num::<u32>(0)?);
                put(&mut w, a.num::<u64>(1)?);
                put(&mut w, a.num::<u64>(2)?);
                (TYPE_HARDWARE, 3)
            }
            "VenHw" | "VenMsg" | "VenMedia" => {
                a.range(1, 2)?;
                w.extend(a.guid(0)?.to_bytes());
                if args.len() == 2 {
                    w.extend(a.hex(1)?);
                }
                match name {
                    "VenHw" => (TYPE_HARDWARE, 4),
                    "VenMsg" => (TYPE_MESSAGING, 10),
                    _ => (TYPE_MEDIA, 3),
                }
            }
            "Ctrl" => {
                a.count(1)?;
                put(&mut w, a.num::<u32>(0)?);
                (TYPE_HARDWARE, 5)
            }
            "PciRoot" | "PcieRoot" | "Floppy" => {
                a.count(1)?;
                let hid: u32 = match name {
                    "PciRoot" => 0x0a03_41d0,
                    "PcieRoot" => 0x0a08_41d0,
                    _ => 0x0604_41d0,
                };
                put(&mut w, hid);
                put(&mut w, a.num::<u32>(0)?);
                (TYPE_ACPI, 1)
            }
            "Acpi" => {
                a.count(2)?;
                put(&mut w, a.eisa(0)?);
                put(&mut w, a.num::<u32>(1)?);
                (TYPE_ACPI, 1)
            }
            "AcpiEx" => {
                // Text order is HID,CID,UID; binary order is HID,UID,CID then
                // three NUL-terminated strings (empty here).
                a.count(3)?;
                put(&mut w, a.eisa(0)?);
                put(&mut w, a.num::<u32>(2)?);
                put(&mut w, a.eisa(1)?);
                w.extend([0, 0, 0]);
                (TYPE_ACPI, 2)
            }
            "AcpiAdr" => {
                if args.is_empty() {
                    return Err("AcpiAdr needs at least one address".into());
                }
                for i in 0..args.len() {
                    put(&mut w, a.num::<u32>(i)?);
                }
                (TYPE_ACPI, 3)
            }
            "Ata" => {
                a.count(3)?;
                let primary = a.word(0, &["Primary", "Secondary"])?;
                let master = a.word(1, &["Master", "Slave"])?;
                w.extend([primary, master]);
                put(&mut w, a.num::<u16>(2)?);
                (TYPE_MESSAGING, 1)
            }
            "Scsi" => {
                a.count(2)?;
                put(&mut w, a.num::<u16>(0)?);
                put(&mut w, a.num::<u16>(1)?);
                (TYPE_MESSAGING, 2)
            }
            "USB" => {
                a.count(2)?;
                w.extend([a.num::<u8>(0)?, a.num::<u8>(1)?]);
                (TYPE_MESSAGING, 5)
            }
            "MAC" => {
                a.count(2)?;
                let mac = a.hex(0)?;
                if mac.len() > 32 {
                    return Err("MAC address is longer than 32 bytes".into());
                }
                w.extend(&mac);
                w.resize(32, 0);
                w.push(a.num::<u8>(1)?);
                (TYPE_MESSAGING, 11)
            }
            "Unit" => {
                a.count(1)?;
                w.push(a.num::<u8>(0)?);
                (TYPE_MESSAGING, 17)
            }
            "Sata" => {
                a.count(3)?;
                for i in 0..3 {
                    put(&mut w, a.num::<u16>(i)?);
                }
                (TYPE_MESSAGING, 18)
            }
            "Vlan" => {
                a.count(1)?;
                put(&mut w, a.num::<u16>(0)?);
                (TYPE_MESSAGING, 20)
            }
            "NVMe" => {
                a.count(2)?;
                put(&mut w, a.num::<u32>(0)?);
                let eui = a.hex(1)?;
                if eui.len() != 8 {
                    return Err("NVMe EUI-64 must be 8 bytes, e.g. 00-00-00-00-00-00-00-00".into());
                }
                w.extend(eui);
                (TYPE_MESSAGING, 23)
            }
            "Uri" => {
                w.extend(inner.as_bytes());
                (TYPE_MESSAGING, 24)
            }
            "SD" | "eMMC" => {
                a.count(1)?;
                w.push(a.num::<u8>(0)?);
                (TYPE_MESSAGING, if name == "SD" { 26 } else { 29 })
            }
            "Wi-Fi" => {
                if inner.len() > 32 {
                    return Err("Wi-Fi SSID is longer than 32 bytes".into());
                }
                w.extend(inner.as_bytes());
                w.resize(32, 0);
                (TYPE_MESSAGING, 28)
            }
            "HD" => {
                a.count(5)?;
                put(&mut w, a.num::<u32>(0)?);
                put(&mut w, a.num::<u64>(3)?);
                put(&mut w, a.num::<u64>(4)?);
                let (sig, format, sig_type) = match a.get(1)? {
                    "GPT" => (a.guid(2)?.to_bytes(), 2u8, 2u8),
                    "MBR" => {
                        let mut sig = [0u8; 16];
                        sig[..4].copy_from_slice(&a.num::<u32>(2)?.to_le_bytes());
                        (sig, 1, 1)
                    }
                    other => return Err(format!("HD partition format must be GPT or MBR, not {other:?}")),
                };
                w.extend(sig);
                w.extend([format, sig_type]);
                (TYPE_MEDIA, 1)
            }
            "CDROM" => {
                a.count(3)?;
                put(&mut w, a.num::<u32>(0)?);
                put(&mut w, a.num::<u64>(1)?);
                put(&mut w, a.num::<u64>(2)?);
                (TYPE_MEDIA, 2)
            }
            "Media" | "FvFile" | "Fv" => {
                a.count(1)?;
                w.extend(a.guid(0)?.to_bytes());
                (TYPE_MEDIA, match name {
                    "Media" => 5,
                    "FvFile" => 6,
                    _ => 7,
                })
            }
            "Offset" => {
                a.count(2)?;
                put(&mut w, 0u32);
                put(&mut w, a.num::<u64>(0)?);
                put(&mut w, a.num::<u64>(1)?);
                (TYPE_MEDIA, 8)
            }
            "RamDisk" => {
                a.count(4)?;
                put(&mut w, a.num::<u64>(0)?);
                put(&mut w, a.num::<u64>(1)?);
                w.extend(a.guid(3)?.to_bytes());
                put(&mut w, a.num::<u16>(2)?);
                (TYPE_MEDIA, 9)
            }
            "BBS" => {
                // The description may itself contain commas.
                if args.len() < 3 {
                    return Err("BBS takes 3 arguments: BBS(type,description,flags)".into());
                }
                put(&mut w, a.num::<u16>(0)?);
                put(&mut w, a.num::<u16>(args.len() - 1)?);
                // Take the description verbatim from between the first and
                // last commas so its internal spacing is preserved.
                let desc = &inner[inner.find(',').unwrap() + 1..inner.rfind(',').unwrap()];
                if !desc.is_ascii() || desc.contains('\0') {
                    return Err("BBS description must be plain ASCII".into());
                }
                w.extend(desc.as_bytes());
                w.push(0);
                (TYPE_BBS, 1)
            }
            _ => return Err(format!("unknown node type {name:?}; use Path(type,subtype,hex) for unsupported nodes")),
        };
        Ok(Node { kind, sub_type, data: w })
    }
}

/// Split `Name(args)` into name and the text between the outer parentheses.
/// Returns `None` for text that is not a call (i.e. a file path).
fn split_call(text: &str) -> Result<Option<(&str, &str)>, String> {
    let Some(open) = text.find('(') else { return Ok(None) };
    let name = &text[..open];
    let is_name = !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !is_name {
        return Ok(None);
    }
    if !text.ends_with(')') {
        return Err(format!("{name}( is missing its closing ')'"));
    }
    Ok(Some((name, &text[open + 1..text.len() - 1])))
}

fn put<T: ToLeBytes>(w: &mut Vec<u8>, v: T) {
    w.extend(v.to_le());
}

trait ToLeBytes {
    fn to_le(self) -> Vec<u8>;
}

macro_rules! impl_le {
    ($($t:ty),*) => {$(impl ToLeBytes for $t { fn to_le(self) -> Vec<u8> { self.to_le_bytes().to_vec() } })*};
}
impl_le!(u16, u32, u64);

/// Argument accessors that produce helpful error messages.
struct Args<'a> {
    name: &'a str,
    args: &'a [&'a str],
}

impl Args<'_> {
    fn count(&self, n: usize) -> Result<(), String> {
        self.range(n, n)
    }

    fn range(&self, min: usize, max: usize) -> Result<(), String> {
        let n = self.args.len();
        if n < min || n > max {
            let want = if min == max { format!("{min}") } else { format!("{min}-{max}") };
            return Err(format!("{} takes {want} argument(s), got {n}", self.name));
        }
        Ok(())
    }

    fn get(&self, i: usize) -> Result<&str, String> {
        self.args.get(i).copied().ok_or_else(|| format!("{} is missing argument {}", self.name, i + 1))
    }

    /// `0x`-prefixed hex or decimal, range-checked for `T`.
    fn num<T: TryFrom<u64>>(&self, i: usize) -> Result<T, String> {
        let s = self.get(i)?;
        let v = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            Some(hex) => u64::from_str_radix(hex, 16),
            None => s.parse::<u64>(),
        }
        .map_err(|_| format!("{}: {s:?} is not a number (use decimal or 0x-prefixed hex)", self.name))?;
        T::try_from(v).map_err(|_| format!("{}: {s} is out of range for argument {}", self.name, i + 1))
    }

    fn hex(&self, i: usize) -> Result<Vec<u8>, String> {
        let s: String = self.get(i)?.chars().filter(|c| *c != '-' && *c != ':').collect();
        if s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{}: {:?} is not a hex byte string", self.name, self.args[i]));
        }
        Ok((0..s.len()).step_by(2).map(|j| u8::from_str_radix(&s[j..j + 2], 16).unwrap()).collect())
    }

    fn guid(&self, i: usize) -> Result<Guid, String> {
        let s = self.get(i)?;
        s.trim_start_matches('{')
            .trim_end_matches('}')
            .parse()
            .map_err(|_| format!("{}: {s:?} is not a GUID", self.name))
    }

    /// EISA PnP id such as `PNP0A03`, or a plain number.
    fn eisa(&self, i: usize) -> Result<u32, String> {
        let s = self.get(i)?;
        let b = s.as_bytes();
        if b.len() == 7 && b[..3].iter().all(u8::is_ascii_uppercase) {
            let product = u16::from_str_radix(&s[3..], 16)
                .map_err(|_| format!("{}: {s:?} is not an EISA id like PNP0A03", self.name))?;
            let c = |x: u8| u32::from(x - b'@');
            return Ok((u32::from(product) << 16) | (c(b[0]) << 10) | (c(b[1]) << 5) | c(b[2]));
        }
        self.num(i)
    }

    /// One of `words` (its index), or a number.
    fn word(&self, i: usize, words: &[&str]) -> Result<u8, String> {
        let s = self.get(i)?;
        match words.iter().position(|w| w.eq_ignore_ascii_case(s)) {
            Some(p) => Ok(p as u8),
            None => self.num(i),
        }
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
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

fn vendor(prefix: &str, r: &Reader, full: bool) -> Option<String> {
    let guid = r.guid(0)?;
    let rest = &r.0[16..];
    Some(if rest.is_empty() {
        format!("{prefix}({guid})")
    } else if full {
        format!("{prefix}({guid},{})", crate::hex::to_hex(rest))
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
    fn text_forms_round_trip() {
        let forms = [
            "Pci(0x14,0x0)",
            "PcCard(0x1)",
            "MemoryMapped(0xb,0x1000,0x1fff)",
            "VenHw(01234567-89ab-cdef-0123-456789abcdef)",
            "VenMsg(01234567-89ab-cdef-0123-456789abcdef,00ff10)",
            "Ctrl(0x0)",
            "PciRoot(0x0)",
            "PcieRoot(0x1)",
            "Acpi(PNP0501,0x1)",
            "AcpiEx(PNP0A08,PNP0A03,0x0)",
            "AcpiAdr(0x80010302,0x80010303)",
            "Ata(Secondary,Master,0)",
            "Scsi(0x2,0x0)",
            "USB(0x3,0x0)",
            "MAC(0022446688aa,0x1)",
            "Unit(0x1)",
            "Sata(0x0,0xffff,0x0)",
            "Vlan(42)",
            "NVMe(0x1,00-25-38-B5-21-A0-0F-21)",
            "Uri(http://example.com/boot.efi)",
            "SD(0x0)",
            "eMMC(0x1)",
            "Wi-Fi(home, network)",
            "HD(1,GPT,01234567-89ab-cdef-0123-456789abcdef,0x800,0x100000)",
            "HD(2,MBR,0x1234abcd,0x800,0x100000)",
            "CDROM(0x0,0x0,0x1000)",
            "\\EFI\\BOOT\\BOOTX64.EFI",
            "Media(01234567-89ab-cdef-0123-456789abcdef)",
            "FvFile(01234567-89ab-cdef-0123-456789abcdef)",
            "Fv(01234567-89ab-cdef-0123-456789abcdef)",
            "Offset(0x100,0x200)",
            "RamDisk(0x1000,0x2000,0,01234567-89ab-cdef-0123-456789abcdef)",
            "BBS(0x5,Hard, Drive,0x0)",
            "Path(9,9,aabb)",
        ];
        for text in forms {
            let node = Node::from_text(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(node.to_text(), text, "display of {text}");
            assert_eq!(node.edit_text(), text, "edit text of {text}");
            assert_eq!(Node::from_text(&node.edit_text()).unwrap(), node);
        }
    }

    #[test]
    fn edit_text_falls_back_to_generic_when_lossy() {
        // IPv4 is displayed in abbreviated form, so editing uses Path().
        let ipv4 = Node { kind: TYPE_MESSAGING, sub_type: 12, data: vec![1; 27] };
        assert!(ipv4.to_text().starts_with("IPv4("));
        assert_eq!(ipv4.edit_text(), ipv4.generic_text());
        assert_eq!(Node::from_text(&ipv4.edit_text()).unwrap(), ipv4);
        // Long vendor data is truncated for display but complete for editing.
        let ven = Node { kind: TYPE_HARDWARE, sub_type: 4, data: vec![7; 64] };
        assert!(ven.to_text().contains('…'));
        assert_eq!(Node::from_text(&ven.edit_text()).unwrap(), ven);
    }

    #[test]
    fn friendly_input_variants() {
        let n = |s| Node::from_text(s).unwrap();
        assert_eq!(n(" Pci( 20 , 0 ) "), n("Pci(0x14,0x0)"));
        assert_eq!(n("Acpi(0x0a0341d0,0)"), n("PciRoot(0x0)"));
        assert_eq!(n("VenHw({01234567-89ab-cdef-0123-456789abcdef})"), n("VenHw(01234567-89ab-cdef-0123-456789abcdef)"));
        assert_eq!(n("Ata(primary,slave,0)"), n("Ata(0,1,0)"));
    }

    #[test]
    fn text_errors_are_descriptive() {
        let err = |s| Node::from_text(s).unwrap_err();
        assert!(err("Pci(0x14)").contains("takes 2"));
        assert!(err("Pci(0x14,0x100)").contains("out of range"));
        assert!(err("Pci(0x1,zz)").contains("not a number"));
        assert!(err("Pci(0x1,0x0").contains("missing its closing"));
        assert!(err("Bogus(1)").contains("unknown node type"));
        assert!(err("HD(1,FAT,0,0,0)").contains("GPT or MBR"));
        assert!(err("Path(127,1,)").contains("End nodes"));
        assert!(err("").contains("empty"));
    }

    #[test]
    fn to_bytes_round_trips_and_validates() {
        let bytes = sample_path();
        assert_eq!(DevicePath::parse_exact(&bytes).unwrap().to_bytes().unwrap(), bytes);
        let multi = [
            node(TYPE_ACPI, 1, &[0xd0, 0x41, 0x03, 0x0a, 0, 0, 0, 0]),
            node(TYPE_END, END_INSTANCE, &[]),
            node(TYPE_MESSAGING, 5, &[1, 0]),
            node(TYPE_END, END_ENTIRE, &[]),
        ]
        .concat();
        let path = DevicePath::parse_exact(&multi).unwrap();
        assert_eq!(path.instances.len(), 2);
        assert_eq!(path.to_bytes().unwrap(), multi);
        // Trailing data after End-Entire cannot be edited structurally.
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(DevicePath::parse_exact(&trailing).is_err());
        assert!(DevicePath { instances: vec![vec![], vec![]] }.to_bytes().is_err());
        assert_eq!(DevicePath { instances: vec![vec![]] }.to_bytes().unwrap(), [TYPE_END, END_ENTIRE, 4, 0]);
    }

    #[test]
    fn rejects_bad_length() {
        assert_eq!(DevicePath::parse(&[1, 1, 2, 0]), Err(Error::BadLength { offset: 0, len: 2 }));
        assert_eq!(DevicePath::parse(&[1, 1, 9, 0]), Err(Error::Truncated { offset: 0 }));
    }
}
