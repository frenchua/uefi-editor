//! Registry mapping the variables defined by the UEFI 2.11 specification to
//! parsers, producing a display tree. Vendor-specific variables are not
//! decoded.

use crate::guids::{
    self, EFI_CAPSULE_REPORT, EFI_DEVICE_SECURITY_DATABASE, EFI_GLOBAL_VARIABLE, EFI_HARDWARE_ERROR_VARIABLE,
    EFI_IMAGE_SECURITY_DATABASE,
};
use crate::key_option::KeyOption;
use crate::load_option::{self, LoadOption, is_numbered};
use crate::siglist::SignatureDatabase;
use crate::{Guid, Variable, capsule, cper, devpath, hex};

/// A labelled value with optional children, rendered as an indented tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub value: String,
    pub children: Vec<Field>,
}

impl Field {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self { label: label.into(), value: value.into(), children: Vec::new() }
    }

    pub fn with(mut self, children: Vec<Field>) -> Self {
        self.children = children;
        self
    }

    /// Depth-first flatten into `(depth, field)` pairs.
    pub fn flatten<'a>(fields: &'a [Field], depth: usize, out: &mut Vec<(usize, &'a Field)>) {
        for f in fields {
            out.push((depth, f));
            Field::flatten(&f.children, depth + 1, out);
        }
    }
}

/// Which structure a variable holds; drives both display and structured editing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    LoadOption,
    OptionOrder,
    OptionNumber,
    Seconds,
    Bool,
    Ascii,
    DevicePath,
    OsIndications,
    BootOptionSupport,
    KeyOption,
    SignatureDatabase,
    GuidList,
    HwErrRecSupport,
    /// Common Platform Error Record (`HwErrRec####`).
    Cper,
    CapsuleResult,
    /// `CapsuleMax` / `CapsuleLast`: CHAR16[11] variable name.
    CapsuleName,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::LoadOption => "EFI_LOAD_OPTION",
            Kind::OptionOrder => "UINT16[] option order",
            Kind::OptionNumber => "UINT16 option number",
            Kind::Seconds => "UINT16 seconds",
            Kind::Bool => "UINT8 boolean",
            Kind::Ascii => "ASCII string",
            Kind::DevicePath => "EFI_DEVICE_PATH",
            Kind::OsIndications => "UINT64 OsIndications bitmask",
            Kind::BootOptionSupport => "UINT32 BootOptionSupport bitmask",
            Kind::KeyOption => "EFI_KEY_OPTION",
            Kind::SignatureDatabase => "EFI_SIGNATURE_LIST[]",
            Kind::GuidList => "EFI_GUID[]",
            Kind::HwErrRecSupport => "UINT16 error record support level",
            Kind::Cper => "Common Platform Error Record",
            Kind::CapsuleResult => "EFI_CAPSULE_RESULT_VARIABLE_HEADER",
            Kind::CapsuleName => "CHAR16[11] capsule variable name",
        }
    }
}

/// Identify the structure of a variable from its vendor GUID and name.
/// Only variables defined by the UEFI 2.11 specification are recognized.
pub fn classify(vendor: &Guid, name: &str) -> Option<Kind> {
    // OS recovery options live in OS vendor namespaces (§3.4.1).
    if is_numbered(name, "OsRecovery") {
        return Some(Kind::LoadOption);
    }
    match *vendor {
        EFI_IMAGE_SECURITY_DATABASE => {
            return matches!(name, "db" | "dbx" | "dbt" | "dbr").then_some(Kind::SignatureDatabase);
        }
        EFI_DEVICE_SECURITY_DATABASE => return (name == "devdb").then_some(Kind::SignatureDatabase),
        EFI_HARDWARE_ERROR_VARIABLE => return is_numbered(name, "HwErrRec").then_some(Kind::Cper),
        EFI_CAPSULE_REPORT => {
            return match name {
                "CapsuleMax" | "CapsuleLast" => Some(Kind::CapsuleName),
                _ if is_numbered(name, "Capsule") => Some(Kind::CapsuleResult),
                _ => None,
            };
        }
        EFI_GLOBAL_VARIABLE => {}
        _ => return None,
    }
    Some(match name {
        _ if ["Boot", "Driver", "SysPrep", "PlatformRecovery"].iter().any(|p| is_numbered(name, p)) => {
            Kind::LoadOption
        }
        _ if is_numbered(name, "Key") => Kind::KeyOption,
        "BootOrder" | "DriverOrder" | "SysPrepOrder" => Kind::OptionOrder,
        "BootCurrent" | "BootNext" => Kind::OptionNumber,
        "Timeout" => Kind::Seconds,
        "SecureBoot" | "SetupMode" | "AuditMode" | "DeployedMode" | "VendorKeys" | "devAuthBoot" => Kind::Bool,
        "PlatformLang" | "PlatformLangCodes" => Kind::Ascii,
        "ConIn" | "ConOut" | "ErrOut" | "ConInDev" | "ConOutDev" | "ErrOutDev" => Kind::DevicePath,
        "OsIndications" | "OsIndicationsSupported" => Kind::OsIndications,
        "BootOptionSupport" => Kind::BootOptionSupport,
        "HwErrRecSupport" => Kind::HwErrRecSupport,
        "PK" | "KEK" | "PKDefault" | "KEKDefault" | "dbDefault" | "dbxDefault" | "dbtDefault" | "dbrDefault"
        | "devdbDefault" => Kind::SignatureDatabase,
        "SignatureSupport" | "OsRecoveryOrder" => Kind::GuidList,
        _ => return None,
    })
}

/// Load-option variable prefix referenced by an order list or option number
/// (`DriverOrder` lists `Driver####` entries, and so on).
pub fn option_prefix(name: &str) -> &'static str {
    match name {
        "DriverOrder" => "Driver",
        "SysPrepOrder" => "SysPrep",
        _ => "Boot",
    }
}

/// `EFI_OS_INDICATIONS_*` bits (§8.5.4).
pub const OS_INDICATIONS: &[(u64, &str)] = &[
    (0x01, "BOOT_TO_FW_UI"),
    (0x02, "TIMESTAMP_REVOCATION"),
    (0x04, "FILE_CAPSULE_DELIVERY_SUPPORTED"),
    (0x08, "FMP_CAPSULE_SUPPORTED"),
    (0x10, "CAPSULE_RESULT_VAR_SUPPORTED"),
    (0x20, "START_OS_RECOVERY"),
    (0x40, "START_PLATFORM_RECOVERY"),
    (0x80, "JSON_CONFIG_DATA_REFRESH"),
];

/// Decoded view of a variable.
pub struct Decoded {
    pub kind: Kind,
    pub fields: Vec<Field>,
}

/// `None` if the variable is not a known structure; `Some(Err)` if it is but
/// its bytes failed to parse.
pub fn decode(var: &Variable) -> Option<Result<Decoded, String>> {
    let kind = classify(&var.key.vendor, &var.key.name)?;
    Some(decode_as(kind, &var.key.name, &var.data).map(|fields| Decoded { kind, fields }))
}

fn decode_as(kind: Kind, name: &str, data: &[u8]) -> Result<Vec<Field>, String> {
    let prefix = option_prefix(name);
    Ok(match kind {
        Kind::LoadOption => load_option_fields(&LoadOption::parse(data).map_err(|e| e.to_string())?),
        Kind::OptionOrder => load_option::parse_order(data)?
            .iter()
            .enumerate()
            .map(|(i, n)| Field::new(format!("[{i}]"), format!("{prefix}{n:04X}")))
            .collect(),
        Kind::OptionNumber => {
            vec![Field::new("Option", format!("{prefix}{:04X}", u16::from_le_bytes(fixed::<2>(data)?)))]
        }
        Kind::HwErrRecSupport => {
            let v = u16::from_le_bytes(fixed::<2>(data)?);
            vec![Field::new("Level", match v {
                0 => "0 (no support)".to_string(),
                1 => "1 (supported, UEFI §8.2.7)".to_string(),
                n => format!("{n} (reserved)"),
            })]
        }
        Kind::Cper => cper::fields(data)?,
        Kind::CapsuleResult => capsule::result_fields(data)?,
        Kind::CapsuleName => capsule::name_fields(data)?,
        Kind::Seconds => {
            let secs = u16::from_le_bytes(fixed::<2>(data)?);
            let text = match secs {
                0 => "0 (boot immediately)".into(),
                0xffff => "0xFFFF (wait forever)".into(),
                s => format!("{s} s"),
            };
            vec![Field::new("Timeout", text)]
        }
        Kind::Bool => {
            let [b] = fixed::<1>(data)?;
            vec![Field::new("Value", match b {
                0 => "0 (disabled)".to_string(),
                1 => "1 (enabled)".to_string(),
                n => format!("{n} (non-boolean)"),
            })]
        }
        Kind::Ascii => {
            let s = String::from_utf8_lossy(data.split(|&b| b == 0).next().unwrap_or_default()).into_owned();
            vec![Field::new("Value", s)]
        }
        Kind::DevicePath => device_path_fields(data)?,
        Kind::OsIndications => {
            let v = u64::from_le_bytes(fixed::<8>(data)?);
            let mut fields = vec![Field::new("Value", format!("0x{v:016x}"))];
            fields.extend(OS_INDICATIONS.iter().filter(|(bit, _)| v & bit != 0).map(|(_, n)| Field::new("set", *n)));
            fields
        }
        Kind::BootOptionSupport => {
            let v = u32::from_le_bytes(fixed::<4>(data)?);
            vec![
                Field::new("Value", format!("0x{v:08x}")),
                Field::new("KEY", (v & 0x1 != 0).to_string()),
                Field::new("APP", (v & 0x2 != 0).to_string()),
                Field::new("SYSPREP", (v & 0x10 != 0).to_string()),
                Field::new("Max key count", ((v >> 8) & 0x3).to_string()),
            ]
        }
        Kind::KeyOption => {
            let k = KeyOption::parse(data).map_err(|e| e.to_string())?;
            let mut fields = vec![
                Field::new("Boot option", format!("Boot{:04X}", k.boot_option)),
                Field::new("Option CRC32", format!("0x{:08x}", k.boot_option_crc)),
                Field::new("Modifiers", k.modifiers()),
            ];
            fields.extend(k.keys.iter().enumerate().map(|(i, key)| Field::new(format!("Key[{i}]"), key.to_text())));
            fields
        }
        Kind::SignatureDatabase => signature_fields(&SignatureDatabase::parse(data).map_err(|e| e.to_string())?),
        Kind::GuidList => {
            if data.len() % 16 != 0 {
                return Err(format!("length {} is not a multiple of 16", data.len()));
            }
            data.chunks_exact(16)
                .enumerate()
                .map(|(i, c)| {
                    let g = Guid::from_bytes(c.try_into().unwrap());
                    let label = guids::cert_type_name(&g).or(guids::vendor_name(&g));
                    Field::new(format!("[{i}]"), match label {
                        Some(l) => format!("{g} ({l})"),
                        None => g.to_string(),
                    })
                })
                .collect()
        }
    })
}

fn fixed<const N: usize>(data: &[u8]) -> Result<[u8; N], String> {
    data.try_into().map_err(|_| format!("expected {N} bytes, found {}", data.len()))
}

pub fn load_option_fields(opt: &LoadOption) -> Vec<Field> {
    let mut fields = vec![
        Field::new("Attributes", format!("0x{:08x} {}", opt.attributes, opt.attribute_names())),
        Field::new("Description", opt.description.to_string()),
    ];
    // FilePathList is a packed array of device paths; the first locates the
    // image and any others are OS-specific (§3.1.3).
    match devpath::DevicePath::parse_list(&opt.file_path_list) {
        Ok(paths) => {
            for (i, path) in paths.iter().enumerate() {
                let label = if i == 0 { "Device path".to_string() } else { format!("Device path {i}") };
                let children = device_path_fields(&path.to_bytes().unwrap_or_default()).unwrap_or_default();
                fields.push(Field::new(label, path.to_text()).with(children));
            }
        }
        Err(_) => match device_path_fields(&opt.file_path_list) {
            Ok(children) => fields.push(Field::new("Device path", opt.device_path_text()).with(children)),
            Err(e) => fields.push(Field::new("Device path", e)),
        },
    }
    if !opt.optional_data.is_empty() {
        let ascii: String = opt.optional_data.iter().take(48).map(|&b| hex::printable(b)).collect();
        fields.push(Field::new("Optional data", format!("{} bytes", opt.optional_data.len())).with(vec![
            Field::new("hex", hex::preview(&opt.optional_data, 24)),
            Field::new("ascii", ascii),
        ]));
    }
    fields
}

fn device_path_fields(data: &[u8]) -> Result<Vec<Field>, String> {
    let path = devpath::DevicePath::parse(data).map_err(|e| e.to_string())?;
    let multi = path.instances.len() > 1;
    let node_fields = |nodes: &[devpath::Node]| -> Vec<Field> {
        nodes
            .iter()
            .map(|n| Field::new(format!("{}/{}", n.type_name(), n.sub_type), n.to_text()))
            .collect()
    };
    Ok(if multi {
        path.instances
            .iter()
            .enumerate()
            .map(|(i, nodes)| Field::new(format!("Instance {i}"), "").with(node_fields(nodes)))
            .collect()
    } else {
        path.instances.first().map(|n| node_fields(n)).unwrap_or_default()
    })
}

fn signature_fields(db: &SignatureDatabase) -> Vec<Field> {
    db.lists
        .iter()
        .enumerate()
        .map(|(i, list)| {
            let ty = guids::cert_type_name(&list.signature_type)
                .map(str::to_string)
                .unwrap_or_else(|| list.signature_type.to_string());
            let entries = list
                .signatures
                .iter()
                .enumerate()
                .map(|(j, sig)| signature_entry(j, &list.signature_type, &sig.owner, &sig.data))
                .collect();
            Field::new(format!("List {i}"), format!("{ty}, {} entries", list.signatures.len())).with(entries)
        })
        .collect()
}

fn signature_entry(index: usize, ty: &Guid, owner: &Guid, data: &[u8]) -> Field {
    let owner_name = guids::vendor_name(owner).map(|n| format!(" ({n})")).unwrap_or_default();
    let owner = Field::new("Owner", format!("{owner}{owner_name}"));
    if *ty == guids::EFI_CERT_X509 {
        match x509_parser::parse_x509_certificate(data) {
            Ok((_, cert)) => {
                let subject = cert.subject().to_string();
                return Field::new(format!("[{index}]"), subject.clone()).with(vec![
                    owner,
                    Field::new("Subject", subject),
                    Field::new("Issuer", cert.issuer().to_string()),
                    Field::new("Serial", cert.raw_serial_as_string()),
                    Field::new("Not before", cert.validity().not_before.to_string()),
                    Field::new("Not after", cert.validity().not_after.to_string()),
                ]);
            }
            Err(e) => {
                return Field::new(format!("[{index}]"), format!("unparseable X.509: {e}"))
                    .with(vec![owner, Field::new("Size", format!("{} bytes", data.len()))]);
            }
        }
    }
    Field::new(format!("[{index}]"), hex::to_hex(data)).with(vec![owner])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VarKey;

    fn var(name: &str, vendor: Guid, data: &[u8]) -> Variable {
        Variable { key: VarKey::new(name, vendor), attributes: 7, data: data.to_vec() }
    }

    #[test]
    fn classifies() {
        assert_eq!(classify(&EFI_GLOBAL_VARIABLE, "Boot0003"), Some(Kind::LoadOption));
        assert_eq!(classify(&EFI_GLOBAL_VARIABLE, "BootOrder"), Some(Kind::OptionOrder));
        assert_eq!(classify(&EFI_IMAGE_SECURITY_DATABASE, "dbx"), Some(Kind::SignatureDatabase));
        assert_eq!(classify(&EFI_IMAGE_SECURITY_DATABASE, "BootOrder"), None);
        assert_eq!(classify(&guids::MICROSOFT_VENDOR, "Boot0000"), None);
        assert_eq!(classify(&guids::MICROSOFT_VENDOR, "OsRecovery0001"), Some(Kind::LoadOption));
        assert_eq!(classify(&EFI_HARDWARE_ERROR_VARIABLE, "HwErrRec0001"), Some(Kind::Cper));
        assert_eq!(classify(&EFI_CAPSULE_REPORT, "Capsule0002"), Some(Kind::CapsuleResult));
        assert_eq!(classify(&EFI_CAPSULE_REPORT, "CapsuleLast"), Some(Kind::CapsuleName));
        assert_eq!(classify(&EFI_DEVICE_SECURITY_DATABASE, "devdb"), Some(Kind::SignatureDatabase));
        // Lang/LangCodes were removed from the UEFI 2.11 global variable table.
        assert_eq!(classify(&EFI_GLOBAL_VARIABLE, "Lang"), None);
    }

    #[test]
    fn order_lists_use_their_own_prefix() {
        let d = decode(&var("DriverOrder", EFI_GLOBAL_VARIABLE, &[1, 0])).unwrap().unwrap();
        assert_eq!(d.fields[0].value, "Driver0001");
    }

    #[test]
    fn decodes_simple_values() {
        let d = decode(&var("BootOrder", EFI_GLOBAL_VARIABLE, &[3, 0, 1, 0])).unwrap().unwrap();
        assert_eq!(d.fields[0].value, "Boot0003");
        let d = decode(&var("Timeout", EFI_GLOBAL_VARIABLE, &[5, 0])).unwrap().unwrap();
        assert_eq!(d.fields[0].value, "5 s");
        assert!(decode(&var("Timeout", EFI_GLOBAL_VARIABLE, &[5])).unwrap().is_err());
        assert!(decode(&var("Whatever", EFI_GLOBAL_VARIABLE, &[])).is_none());
    }
}
