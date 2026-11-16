//! Human-readable, editable forms for the writable variables of the UEFI 2.11
//! specification. [`build`] turns a variable's bytes into typed [`Field`]s,
//! and [`encode`] turns edited fields back into bytes. The UI only needs to
//! know how to edit each [`Value`] type, not each variable layout.

use binrw::NullWideString;

use crate::decode::{Kind, OS_INDICATIONS, option_prefix};
use crate::devpath::{self, DevicePath};
use crate::guids::EFI_GLOBAL_VARIABLE;
use crate::key_option::{self, InputKey, KeyOption};
use crate::load_option::{
    self, LOAD_OPTION_ACTIVE, LOAD_OPTION_CATEGORY_APP, LOAD_OPTION_CATEGORY_MASK, LOAD_OPTION_FORCE_RECONNECT,
    LOAD_OPTION_HIDDEN, LoadOption,
};
use crate::{Guid, Variable, hex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlagDef {
    pub mask: u64,
    pub name: String,
    /// Extra context, e.g. "not supported by this firmware".
    pub note: String,
}

impl FlagDef {
    fn new(mask: u64, name: &str) -> Self {
        Self { mask, name: name.into(), note: String::new() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataView {
    Hex,
    /// UCS-2 text; `nul` records whether a terminating NUL is stored.
    Ucs2 { nul: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Uint { value: u64, bits: u32 },
    Bool(bool),
    /// Bitmask with named bits; bits without a name are preserved untouched.
    Flags { value: u64, defs: Vec<FlagDef> },
    Choice { value: u64, options: Vec<(u64, String)> },
    Text { value: String, ascii: bool },
    Data { bytes: Vec<u8>, view: DataView },
    Guid(Guid),
    DevicePath(DevicePath),
    /// Reference to a load option such as `Boot0003`.
    OptionRef { number: u16, prefix: &'static str },
    Key(InputKey),
    List { items: Vec<Value>, template: Box<Value>, max: Option<usize> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    pub label: String,
    pub value: Value,
    pub help: String,
    /// False for values computed on save (e.g. a CRC).
    pub editable: bool,
}

impl Field {
    fn new(label: &str, value: Value, help: &str) -> Self {
        Self { label: label.into(), value, help: help.into(), editable: true }
    }
}

impl Value {
    pub fn display(&self) -> String {
        match self {
            Value::Uint { value, .. } => format!("{value} (0x{value:x})"),
            Value::Bool(b) => if *b { "yes (1)" } else { "no (0)" }.into(),
            Value::Flags { value, defs } => {
                let set: Vec<&str> = defs.iter().filter(|d| value & d.mask != 0).map(|d| d.name.as_str()).collect();
                if set.is_empty() { format!("0x{value:x}") } else { format!("0x{value:x} {}", set.join(" | ")) }
            }
            Value::Choice { value, options } => {
                options.iter().find(|(v, _)| v == value).map_or(format!("0x{value:x}"), |(_, n)| n.clone())
            }
            Value::Text { value, .. } => format!("\"{value}\""),
            Value::Data { bytes, view } => match view {
                _ if bytes.is_empty() => "(empty)".into(),
                DataView::Hex => format!("{} bytes: {}", bytes.len(), hex::preview(bytes, 24)),
                DataView::Ucs2 { .. } => format!("\"{}\" (UCS-2 text)", devpath::utf16_string(bytes)),
            },
            Value::Guid(g) => g.to_string(),
            Value::DevicePath(p) => p.to_text(),
            Value::OptionRef { number, prefix } => format!("{prefix}{number:04X}"),
            Value::Key(k) => k.to_text(),
            Value::List { items, .. } => format!("{} item(s)", items.len()),
        }
    }

    /// Values edited by typing a line of text.
    pub fn is_text_editable(&self) -> bool {
        matches!(
            self,
            Value::Uint { .. }
                | Value::Text { .. }
                | Value::Data { .. }
                | Value::Guid(_)
                | Value::OptionRef { .. }
                | Value::Key(_)
        )
    }

    /// Initial text for the line editor.
    pub fn edit_text(&self) -> String {
        match self {
            Value::Uint { value, .. } => value.to_string(),
            Value::Text { value, .. } => value.clone(),
            Value::Data { bytes, view: DataView::Hex } => hex::to_hex(bytes),
            Value::Data { bytes, view: DataView::Ucs2 { .. } } => devpath::utf16_string(bytes),
            Value::OptionRef { number, .. } => format!("{number:04X}"),
            other => other.display(),
        }
    }

    /// Parse typed text into this value; leaves it unchanged on error.
    pub fn set_text(&mut self, text: &str) -> Result<(), String> {
        let t = text.trim();
        match self {
            Value::Uint { value, bits } => *value = parse_uint(t, *bits)?,
            Value::Text { value, ascii } => {
                if *ascii && !text.is_ascii() {
                    return Err("only ASCII characters are allowed".into());
                }
                check_ucs2(text)?;
                *value = text.to_string();
            }
            Value::Data { bytes, view: DataView::Hex } => *bytes = parse_hex(t)?,
            Value::Data { bytes, view: DataView::Ucs2 { nul } } => {
                check_ucs2(text)?;
                *bytes = text.encode_utf16().chain(nul.then_some(0)).flat_map(u16::to_le_bytes).collect();
            }
            Value::Guid(g) => {
                *g = t.trim_matches(|c| c == '{' || c == '}').parse().map_err(|_| format!("{t:?} is not a GUID"))?;
            }
            Value::OptionRef { number, prefix } => {
                let digits = t.strip_prefix(*prefix).unwrap_or(t);
                *number = u16::from_str_radix(digits, 16)
                    .map_err(|_| format!("{t:?} is not an option number; use 4 hex digits like 0003 or {prefix}0003"))?;
            }
            Value::Key(k) => *k = InputKey::from_text(t)?,
            _ => return Err("this value is not edited as text".into()),
        }
        Ok(())
    }

    /// Flip a boolean; returns false if not a boolean.
    pub fn toggle(&mut self) -> bool {
        if let Value::Bool(b) = self {
            *b = !*b;
            return true;
        }
        false
    }

    /// Step a choice forwards or backwards; returns false if not a choice.
    pub fn cycle(&mut self, forward: bool) -> bool {
        let Value::Choice { value, options } = self else { return false };
        if options.is_empty() {
            return false;
        }
        let i = options.iter().position(|(v, _)| v == value).unwrap_or(0);
        let n = options.len();
        *value = options[if forward { (i + 1) % n } else { (i + n - 1) % n }].0;
        true
    }

    /// Switch a data blob between hex and UCS-2 text.
    pub fn toggle_data_view(&mut self) -> Result<(), String> {
        let Value::Data { bytes, view } = self else { return Err("not a data field".into()) };
        *view = match view {
            DataView::Ucs2 { .. } => DataView::Hex,
            DataView::Hex => ucs2_view(bytes, false).ok_or("this data is not valid UCS-2 text")?,
        };
        Ok(())
    }
}

fn parse_uint(t: &str, bits: u32) -> Result<u64, String> {
    let v = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => u64::from_str_radix(h, 16),
        None => t.parse(),
    }
    .map_err(|_| format!("{t:?} is not a number (use decimal or 0x-prefixed hex)"))?;
    if bits < 64 && v >> bits != 0 {
        return Err(format!("{t} does not fit in {bits} bits (max {})", (1u64 << bits) - 1));
    }
    Ok(v)
}

fn parse_hex(t: &str) -> Result<Vec<u8>, String> {
    let s: String = t.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != ':').collect();
    if s.len() % 2 != 0 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("enter hex bytes, e.g. 01 02 ff".into());
    }
    Ok((0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect())
}

fn check_ucs2(text: &str) -> Result<(), String> {
    match text.chars().find(|c| u32::from(*c) > 0xffff || *c == '\0') {
        Some(c) => Err(format!("{c:?} cannot be stored as UCS-2")),
        None => Ok(()),
    }
}

/// UCS-2 view of `bytes` if it is clean text (optionally NUL-terminated).
/// `strict` also requires printable ASCII, so binary data that happens to be
/// valid UTF-16 is not shown as CJK noise.
fn ucs2_view(bytes: &[u8], strict: bool) -> Option<DataView> {
    if bytes.len() % 2 != 0 || bytes.is_empty() {
        return None;
    }
    let mut units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let nul = units.last() == Some(&0);
    if nul {
        units.pop();
    }
    let text = String::from_utf16(&units).ok()?;
    let ok = text.chars().all(|c| {
        if strict { (' '..='~').contains(&c) } else { c != '\0' && !c.is_control() }
    });
    ok.then_some(DataView::Ucs2 { nul })
}

/// Kinds that have an editable form (device paths use their own editor).
pub fn has_form(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::LoadOption
            | Kind::OptionOrder
            | Kind::OptionNumber
            | Kind::Seconds
            | Kind::Bool
            | Kind::Ascii
            | Kind::OsIndications
            | Kind::KeyOption
    )
}

fn global<'a>(vars: &'a [Variable], name: &str) -> Option<&'a Variable> {
    vars.iter().find(|v| v.key.vendor == EFI_GLOBAL_VARIABLE && v.key.name == name)
}

/// Description of the load option `prefix####`, for showing next to references.
pub fn option_description(vars: &[Variable], prefix: &str, number: u16) -> Option<String> {
    let v = global(vars, &format!("{prefix}{number:04X}"))?;
    LoadOption::parse(&v.data).ok().map(|o| o.description.to_string())
}

fn fixed<const N: usize>(data: &[u8]) -> Result<[u8; N], String> {
    data.try_into().map_err(|_| format!("expected {N} bytes, found {}", data.len()))
}

const KEY_MODIFIERS: [(u64, &str); 6] =
    [(1 << 8, "Shift"), (1 << 9, "Ctrl"), (1 << 10, "Alt"), (1 << 11, "Logo"), (1 << 12, "Menu"), (1 << 13, "SysReq")];

/// Build the editable form for a variable. `vars` supplies context such as
/// `PlatformLangCodes` and the `Boot####` entries that references point to.
pub fn build(kind: Kind, name: &str, data: &[u8], vars: &[Variable]) -> Result<Vec<Field>, String> {
    let prefix = option_prefix(name);
    Ok(match kind {
        Kind::LoadOption => {
            let opt = LoadOption::parse(data).map_err(|e| e.to_string())?;
            let paths = DevicePath::parse_list(&opt.file_path_list)?;
            let attrs = u64::from(opt.attributes);
            let category = attrs & u64::from(LOAD_OPTION_CATEGORY_MASK);
            let mut categories = vec![(0, "Boot".to_string()), (u64::from(LOAD_OPTION_CATEGORY_APP), "Application".to_string())];
            if !categories.iter().any(|(v, _)| *v == category) {
                categories.push((category, format!("reserved 0x{category:x} (ignored by boot manager)")));
            }
            let view = ucs2_view(&opt.optional_data, true).unwrap_or(DataView::Hex);
            vec![
                Field::new(
                    "Attributes",
                    Value::Flags {
                        value: attrs,
                        defs: vec![
                            FlagDef::new(LOAD_OPTION_ACTIVE.into(), "ACTIVE"),
                            FlagDef::new(LOAD_OPTION_FORCE_RECONNECT.into(), "FORCE_RECONNECT"),
                            FlagDef::new(LOAD_OPTION_HIDDEN.into(), "HIDDEN"),
                        ],
                    },
                    "Only ACTIVE options are tried at boot. Undefined bits are preserved.",
                ),
                Field::new("Category", Value::Choice { value: category, options: categories }, "Boot options are part of the normal boot order; applications are launched on demand (e.g. by hot key)."),
                Field::new("Description", Value::Text { value: opt.description.to_string(), ascii: false }, "Name shown in the firmware boot menu."),
                Field::new(
                    "File paths",
                    Value::List {
                        items: paths.into_iter().map(Value::DevicePath).collect(),
                        template: Box::new(Value::DevicePath(DevicePath { instances: vec![vec![]] })),
                        max: None,
                    },
                    "The first device path locates the image; any further paths are OS-specific (UEFI §3.1.3).",
                ),
                Field::new(
                    "Optional data",
                    Value::Data { bytes: opt.optional_data, view },
                    "Passed to the image as load options (e.g. a Linux kernel command line). t toggles hex / UCS-2 text.",
                ),
            ]
        }
        Kind::OptionOrder => {
            let items = load_option::parse_order(data)?.into_iter().map(|n| Value::OptionRef { number: n, prefix }).collect();
            vec![Field::new(
                "Order",
                Value::List { items, template: Box::new(Value::OptionRef { number: 0, prefix }), max: None },
                "Options are tried top to bottom.",
            )]
        }
        Kind::OptionNumber => {
            let number = u16::from_le_bytes(fixed::<2>(data)?);
            vec![Field::new("Option", Value::OptionRef { number, prefix }, "Boot option tried first on the next boot only; firmware deletes BootNext once used.")]
        }
        Kind::Seconds => vec![Field::new(
            "Timeout (seconds)",
            Value::Uint { value: u16::from_le_bytes(fixed::<2>(data)?).into(), bits: 16 },
            "0 boots immediately; 65535 (0xFFFF) waits for user input.",
        )],
        Kind::Bool => {
            let [b] = fixed::<1>(data)?;
            let help = match name {
                "AuditMode" => "Firmware only accepts changes while DeployedMode is 0 (UEFI §32.3).",
                "DeployedMode" => "Firmware only accepts changes while DeployedMode is 0 (UEFI §32.3).",
                _ => "",
            };
            vec![Field::new("Value", Value::Bool(b != 0), help)]
        }
        Kind::Ascii => {
            let current = String::from_utf8_lossy(data.split(|&b| b == 0).next().unwrap_or_default()).into_owned();
            let codes: Vec<String> = global(vars, "PlatformLangCodes")
                .map(|v| {
                    String::from_utf8_lossy(v.data.split(|&b| b == 0).next().unwrap_or_default())
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if codes.is_empty() {
                vec![Field::new("Language", Value::Text { value: current, ascii: true }, "RFC 4646 language code, e.g. en-US.")]
            } else {
                let mut options: Vec<(u64, String)> = codes.into_iter().enumerate().map(|(i, c)| (i as u64, c)).collect();
                let value = match options.iter().find(|(_, c)| *c == current) {
                    Some((i, _)) => *i,
                    None => {
                        options.push((options.len() as u64, current));
                        options.len() as u64 - 1
                    }
                };
                vec![Field::new("Language", Value::Choice { value, options }, "Choices come from PlatformLangCodes.")]
            }
        }
        Kind::OsIndications => {
            let supported = global(vars, "OsIndicationsSupported").and_then(|v| fixed::<8>(&v.data).ok()).map(u64::from_le_bytes);
            let defs = OS_INDICATIONS
                .iter()
                .map(|(mask, n)| FlagDef {
                    mask: *mask,
                    name: (*n).into(),
                    note: match supported {
                        Some(s) if s & mask == 0 => "not supported by this firmware".into(),
                        _ => String::new(),
                    },
                })
                .collect();
            vec![Field::new(
                "Requests",
                Value::Flags { value: u64::from_le_bytes(fixed::<8>(data)?), defs },
                "Requests for firmware on the next boot, e.g. BOOT_TO_FW_UI enters setup.",
            )]
        }
        Kind::KeyOption => {
            let k = KeyOption::parse(data).map_err(|e| e.to_string())?;
            let target = format!("Boot{:04X}", k.boot_option);
            let crc_note = match global(vars, &target) {
                Some(v) if key_option::crc32(&v.data) == k.boot_option_crc => format!("Matches {target}. Recomputed on save."),
                Some(_) => format!("Does not match {target}, so firmware ignores this key; recomputed on save."),
                None => format!("{target} does not exist."),
            };
            let mut crc = Field::new("Option CRC32", Value::Uint { value: k.boot_option_crc.into(), bits: 32 }, &crc_note);
            crc.editable = false;
            vec![
                Field::new(
                    "Modifiers",
                    Value::Flags {
                        value: u64::from(k.key_data),
                        defs: KEY_MODIFIERS.iter().map(|(m, n)| FlagDef::new(*m, n)).collect(),
                    },
                    "Modifier keys that must be held.",
                ),
                Field::new("Boot option", Value::OptionRef { number: k.boot_option, prefix: "Boot" }, "Boot#### launched by this hot key."),
                Field::new(
                    "Keys",
                    Value::List {
                        items: k.keys.into_iter().map(Value::Key).collect(),
                        template: Box::new(Value::Key(InputKey { scan_code: 0x0b, unicode_char: 0 })),
                        max: Some(key_option::MAX_KEYS),
                    },
                    "Up to 3 keys: names like F12 or Esc, a character like 'p', or scan=0x..,char=0x...",
                ),
                crc,
            ]
        }
        _ => return Err(format!("{} has no editable form", kind.name())),
    })
}

fn field<'a>(fields: &'a [Field], label: &str) -> Result<&'a Value, String> {
    fields.iter().find(|f| f.label == label).map(|f| &f.value).ok_or_else(|| format!("missing field {label}"))
}

/// Encode edited fields back into variable data.
pub fn encode(kind: Kind, fields: &[Field], vars: &[Variable]) -> Result<Vec<u8>, String> {
    let bad = |label: &str| format!("field {label} has the wrong type");
    Ok(match kind {
        Kind::LoadOption => {
            let Value::Flags { value: attrs, .. } = field(fields, "Attributes")? else { return Err(bad("Attributes")) };
            let Value::Choice { value: category, .. } = field(fields, "Category")? else { return Err(bad("Category")) };
            let Value::Text { value: description, .. } = field(fields, "Description")? else { return Err(bad("Description")) };
            let Value::List { items, .. } = field(fields, "File paths")? else { return Err(bad("File paths")) };
            let Value::Data { bytes: optional_data, .. } = field(fields, "Optional data")? else { return Err(bad("Optional data")) };
            let paths: Vec<DevicePath> = items
                .iter()
                .map(|v| match v {
                    Value::DevicePath(p) => Ok(p.clone()),
                    _ => Err(bad("File paths")),
                })
                .collect::<Result<_, _>>()?;
            if paths.is_empty() {
                return Err("a load option needs at least one file path".into());
            }
            let attributes = (*attrs as u32 & !LOAD_OPTION_CATEGORY_MASK) | (*category as u32 & LOAD_OPTION_CATEGORY_MASK);
            LoadOption {
                attributes,
                description: NullWideString::from(description.as_str()),
                file_path_list: DevicePath::list_to_bytes(&paths)?,
                optional_data: optional_data.clone(),
            }
            .to_bytes()
            .map_err(|e| e.to_string())?
        }
        Kind::OptionOrder => {
            let Value::List { items, .. } = field(fields, "Order")? else { return Err(bad("Order")) };
            let mut order = Vec::new();
            for item in items {
                let Value::OptionRef { number, prefix } = item else { return Err(bad("Order")) };
                if order.contains(number) {
                    return Err(format!("{prefix}{number:04X} is listed twice"));
                }
                order.push(*number);
            }
            load_option::encode_order(&order)
        }
        Kind::OptionNumber => {
            let Value::OptionRef { number, .. } = field(fields, "Option")? else { return Err(bad("Option")) };
            number.to_le_bytes().to_vec()
        }
        Kind::Seconds => {
            let Value::Uint { value, .. } = field(fields, "Timeout (seconds)")? else { return Err(bad("Timeout")) };
            (*value as u16).to_le_bytes().to_vec()
        }
        Kind::Bool => {
            let Value::Bool(b) = field(fields, "Value")? else { return Err(bad("Value")) };
            vec![u8::from(*b)]
        }
        Kind::Ascii => {
            let text = match field(fields, "Language")? {
                Value::Text { value, .. } => value.clone(),
                Value::Choice { value, options } => {
                    options.iter().find(|(v, _)| v == value).map(|(_, n)| n.clone()).ok_or(bad("Language"))?
                }
                _ => return Err(bad("Language")),
            };
            // Null-terminated ASCII (UEFI §3.3).
            text.into_bytes().into_iter().chain([0]).collect()
        }
        Kind::OsIndications => {
            let Value::Flags { value, .. } = field(fields, "Requests")? else { return Err(bad("Requests")) };
            value.to_le_bytes().to_vec()
        }
        Kind::KeyOption => {
            let Value::Flags { value: key_data, .. } = field(fields, "Modifiers")? else { return Err(bad("Modifiers")) };
            let Value::OptionRef { number, .. } = field(fields, "Boot option")? else { return Err(bad("Boot option")) };
            let Value::List { items, .. } = field(fields, "Keys")? else { return Err(bad("Keys")) };
            let keys: Vec<InputKey> = items
                .iter()
                .map(|v| match v {
                    Value::Key(k) => Ok(*k),
                    _ => Err(bad("Keys")),
                })
                .collect::<Result<_, _>>()?;
            if keys.len() > key_option::MAX_KEYS {
                return Err(format!("at most {} keys are allowed", key_option::MAX_KEYS));
            }
            let target = format!("Boot{number:04X}");
            let boot = global(vars, &target).ok_or_else(|| format!("{target} does not exist, so its CRC cannot be computed"))?;
            let key_data = (*key_data as u32 & !key_option::KEY_COUNT_MASK) | ((keys.len() as u32) << 30);
            KeyOption { key_data, boot_option_crc: key_option::crc32(&boot.data), boot_option: *number, keys }
                .to_bytes()
                .map_err(|e| e.to_string())?
        }
        _ => return Err(format!("{} has no editable form", kind.name())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VarKey;
    use crate::decode::classify;
    use crate::devpath::Node;

    fn var(name: &str, data: Vec<u8>) -> Variable {
        Variable { key: VarKey::new(name, EFI_GLOBAL_VARIABLE), attributes: 7, data }
    }

    fn boot_option(desc: &str, optional: &[u8]) -> Vec<u8> {
        let path = DevicePath { instances: vec![vec![Node::from_text(r"\EFI\Linux\vmlinuz.efi").unwrap()]] };
        LoadOption {
            attributes: LOAD_OPTION_ACTIVE | 0x20, // 0x20: undefined bit, must be preserved
            description: desc.into(),
            file_path_list: path.to_bytes().unwrap(),
            optional_data: optional.to_vec(),
        }
        .to_bytes()
        .unwrap()
    }

    /// Building a form and encoding it unchanged must reproduce the bytes.
    fn assert_round_trip(v: &Variable, vars: &[Variable]) {
        let kind = classify(&v.key.vendor, &v.key.name).unwrap();
        let fields = build(kind, &v.key.name, &v.data, vars).unwrap();
        assert_eq!(encode(kind, &fields, vars).unwrap(), v.data, "{}", v.key.name);
    }

    fn cmdline(s: &str) -> Vec<u8> {
        s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    }

    fn vars() -> Vec<Variable> {
        let boot = boot_option("Linux", &cmdline("root=/dev/sda1 quiet"));
        let crc = key_option::crc32(&boot);
        let mut key = Vec::new();
        key.extend_from_slice(&((1u32 << 30) | (1 << 9)).to_le_bytes());
        key.extend_from_slice(&crc.to_le_bytes());
        key.extend_from_slice(&1u16.to_le_bytes());
        key.extend_from_slice(&[0x16, 0, 0, 0]);
        vec![
            var("Boot0001", boot),
            var("BootOrder", vec![1, 0, 2, 0]),
            var("BootNext", vec![2, 0]),
            var("Timeout", vec![5, 0]),
            var("AuditMode", vec![0]),
            var("PlatformLang", b"en-US\0".to_vec()),
            var("PlatformLangCodes", b"en-US;fr-FR\0".to_vec()),
            var("OsIndications", 0x41u64.to_le_bytes().to_vec()),
            var("OsIndicationsSupported", 1u64.to_le_bytes().to_vec()),
            var("Key0000", key),
        ]
    }

    #[test]
    fn every_form_round_trips_unchanged() {
        let vars = vars();
        for v in vars.iter().filter(|v| v.key.name != "PlatformLangCodes" && v.key.name != "OsIndicationsSupported") {
            assert_round_trip(v, &vars);
        }
    }

    #[test]
    fn load_option_edits() {
        let vars = vars();
        let mut fields = build(Kind::LoadOption, "Boot0001", &vars[0].data, &vars).unwrap();
        // The Linux command line is shown as text, not hex.
        assert_eq!(fields[4].value.display(), "\"root=/dev/sda1 quiet\" (UCS-2 text)");
        fields[4].value.set_text("root=/dev/sda2").unwrap();
        fields[2].value.set_text("Linux (new)").unwrap();
        let Value::Flags { value, .. } = &mut fields[0].value else { panic!() };
        *value &= !u64::from(LOAD_OPTION_ACTIVE);
        fields[1].value.cycle(true); // Boot -> Application
        let opt = LoadOption::parse(&encode(Kind::LoadOption, &fields, &vars).unwrap()).unwrap();
        assert_eq!(opt.description.to_string(), "Linux (new)");
        assert_eq!(opt.attributes, 0x20 | LOAD_OPTION_CATEGORY_APP, "undefined bit kept, category set");
        assert_eq!(opt.optional_data, cmdline("root=/dev/sda2"));

        let Value::List { items, .. } = &mut fields[3].value else { panic!() };
        items.clear();
        assert!(encode(Kind::LoadOption, &fields, &vars).unwrap_err().contains("at least one file path"));
    }

    #[test]
    fn windows_style_binary_optional_data_stays_hex() {
        let data = boot_option("Windows Boot Manager", b"WINDOWS\0\x01\0\0\0\x88\0");
        let fields = build(Kind::LoadOption, "Boot0000", &data, &[]).unwrap();
        assert!(matches!(fields[4].value, Value::Data { view: DataView::Hex, .. }));
    }

    #[test]
    fn order_rejects_duplicates() {
        let vars = vars();
        let mut fields = build(Kind::OptionOrder, "BootOrder", &vars[1].data, &vars).unwrap();
        let Value::List { items, .. } = &mut fields[0].value else { panic!() };
        items[1].set_text("Boot0001").unwrap();
        assert!(encode(Kind::OptionOrder, &fields, &vars).unwrap_err().contains("listed twice"));
    }

    #[test]
    fn numbers_are_range_checked() {
        let mut v = Value::Uint { value: 5, bits: 16 };
        assert!(v.set_text("70000").unwrap_err().contains("16 bits"));
        v.set_text("0xffff").unwrap();
        assert_eq!(v, Value::Uint { value: 0xffff, bits: 16 });
    }

    #[test]
    fn language_is_a_choice_from_platform_lang_codes() {
        let vars = vars();
        let mut fields = build(Kind::Ascii, "PlatformLang", b"en-US\0", &vars).unwrap();
        assert!(fields[0].value.cycle(true));
        assert_eq!(encode(Kind::Ascii, &fields, &vars).unwrap(), b"fr-FR\0");
    }

    #[test]
    fn os_indications_marks_unsupported_bits() {
        let vars = vars();
        let fields = build(Kind::OsIndications, "OsIndications", &vars[7].data, &vars).unwrap();
        let Value::Flags { defs, .. } = &fields[0].value else { panic!() };
        assert_eq!(defs[0].note, "");
        assert_eq!(defs[1].note, "not supported by this firmware");
    }

    #[test]
    fn key_option_recomputes_crc_for_new_target() {
        let mut vars = vars();
        vars.push(var("Boot0002", boot_option("Other", &[])));
        let key = vars.iter().find(|v| v.key.name == "Key0000").unwrap().data.clone();
        let mut fields = build(Kind::KeyOption, "Key0000", &key, &vars).unwrap();
        assert!(fields[3].help.starts_with("Matches Boot0001"));
        fields[1].value.set_text("0002").unwrap();
        let Value::List { items, .. } = &mut fields[2].value else { panic!() };
        items.push(Value::Key(InputKey::from_text("'r'").unwrap()));
        let k = KeyOption::parse(&encode(Kind::KeyOption, &fields, &vars).unwrap()).unwrap();
        assert_eq!(k.boot_option, 2);
        assert_eq!(k.boot_option_crc, key_option::crc32(&vars.last().unwrap().data));
        assert_eq!(k.keys.len(), 2);
        assert_eq!(k.key_data >> 30, 2);
        assert_eq!(k.key_data & 0x3f00, 1 << 9, "Ctrl kept");

        fields[1].value.set_text("0009").unwrap();
        assert!(encode(Kind::KeyOption, &fields, &vars).unwrap_err().contains("Boot0009 does not exist"));
    }
}
