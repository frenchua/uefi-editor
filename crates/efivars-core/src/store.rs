//! Storage abstraction plus portable in-memory and directory-backed stores.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{Guid, VarKey, Variable};

/// A source and sink of UEFI variables.
pub trait VarStore {
    /// Human-readable description of the backend (shown in the UI).
    fn describe(&self) -> String;
    /// Every readable variable, with values.
    fn list(&mut self) -> io::Result<Vec<Variable>>;
    fn read(&mut self, key: &VarKey) -> io::Result<Variable>;
    /// Create or replace a variable with exactly `var.attributes` and `var.data`.
    fn write(&mut self, var: &Variable) -> io::Result<()>;
    fn delete(&mut self, key: &VarKey) -> io::Result<()>;
}

/// Volatile store, used for tests.
#[derive(Default)]
pub struct MemStore {
    pub vars: Vec<Variable>,
}

impl VarStore for MemStore {
    fn describe(&self) -> String {
        "memory".into()
    }

    fn list(&mut self) -> io::Result<Vec<Variable>> {
        Ok(self.vars.clone())
    }

    fn read(&mut self, key: &VarKey) -> io::Result<Variable> {
        self.vars.iter().find(|v| &v.key == key).cloned().ok_or_else(|| not_found(key))
    }

    fn write(&mut self, var: &Variable) -> io::Result<()> {
        match self.vars.iter_mut().find(|v| v.key == var.key) {
            Some(existing) => *existing = var.clone(),
            None => self.vars.push(var.clone()),
        }
        Ok(())
    }

    fn delete(&mut self, key: &VarKey) -> io::Result<()> {
        let before = self.vars.len();
        self.vars.retain(|v| &v.key != key);
        if self.vars.len() == before { Err(not_found(key)) } else { Ok(()) }
    }
}

/// A directory of files in Linux efivarfs format: each file is named
/// `<Name>-<guid>` and holds a 4-byte little-endian attribute word followed
/// by the value. Used for offline dumps, and by the Linux backend.
///
/// Variable names may contain characters that some filesystems reject (macOS
/// sets `prev-lang:kbd`, and `:` opens an NTFS alternate data stream), so
/// portable stores created with [`DirStore::new`] escape them as `%XX`.
/// [`DirStore::raw`] uses names verbatim, as efivarfs requires.
pub struct DirStore {
    pub root: PathBuf,
    escape: bool,
}

impl DirStore {
    /// Portable dump directory with escaped file names.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), escape: true }
    }

    /// Verbatim file names (Linux efivarfs).
    pub fn raw(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), escape: false }
    }

    pub fn path_for(&self, key: &VarKey) -> io::Result<PathBuf> {
        let unsafe_raw = key.name.contains(['/', '\0']) || key.name == "." || key.name == "..";
        if key.name.is_empty() || (!self.escape && unsafe_raw) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unsafe variable name {:?}", key.name)));
        }
        let name = if self.escape { escape_name(&key.name) } else { key.name.clone() };
        Ok(self.root.join(format!("{name}-{}", key.vendor)))
    }

    /// Parse a file name from this store into a key.
    pub fn parse_file_name(&self, name: &str) -> Option<VarKey> {
        // GUID text is 36 chars, preceded by '-'.
        if name.len() < 38 || !name.is_char_boundary(name.len() - 37) {
            return None;
        }
        let (var, rest) = name.split_at(name.len() - 37);
        let guid: Guid = rest.strip_prefix('-')?.parse().ok()?;
        let var = if self.escape { unescape_name(var)? } else { var.to_string() };
        Some(VarKey::new(var, guid))
    }

    pub fn read_file(path: &Path, key: VarKey) -> io::Result<Variable> {
        let bytes = fs::read(path)?;
        decode_file(key, &bytes)
    }

    /// Bytes as stored on disk: attributes then data.
    pub fn encode_file(var: &Variable) -> Vec<u8> {
        let mut buf = Vec::with_capacity(4 + var.data.len());
        buf.extend_from_slice(&var.attributes.to_le_bytes());
        buf.extend_from_slice(&var.data);
        buf
    }

    /// Write every variable in `vars` to `dir` (creating it).
    pub fn dump(dir: &Path, vars: &[Variable]) -> io::Result<usize> {
        fs::create_dir_all(dir)?;
        let mut store = DirStore::new(dir);
        for v in vars {
            store.write(v)?;
        }
        Ok(vars.len())
    }
}

/// Escape characters that are invalid in Windows/macOS/Linux file names, plus
/// `%` itself and a leading `.`, as `%XX` (UTF-8 bytes).
fn escape_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (i, c) in name.char_indices() {
        let bad = matches!(c, '%' | '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            || c.is_control()
            || (i == 0 && c == '.');
        if bad {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    // Windows strips trailing dots and spaces from file names.
    if let Some(last) = out.pop() {
        if last == '.' || last == ' ' {
            out.push_str(&format!("%{:02X}", last as u8));
        } else {
            out.push(last);
        }
    }
    out
}

fn unescape_name(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub fn decode_file(key: VarKey, bytes: &[u8]) -> io::Result<Variable> {
    if bytes.len() < 4 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{key}: file shorter than attribute header")));
    }
    let attributes = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    Ok(Variable { key, attributes, data: bytes[4..].to_vec() })
}

impl VarStore for DirStore {
    fn describe(&self) -> String {
        format!("directory {}", self.root.display())
    }

    fn list(&mut self) -> io::Result<Vec<Variable>> {
        let mut vars = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(key) = entry.file_name().to_str().and_then(|n| self.parse_file_name(n)) else { continue };
            // Unreadable entries (e.g. root-only on efivarfs) are skipped.
            if let Ok(v) = DirStore::read_file(&entry.path(), key) {
                vars.push(v);
            }
        }
        Ok(vars)
    }

    fn read(&mut self, key: &VarKey) -> io::Result<Variable> {
        DirStore::read_file(&self.path_for(key)?, key.clone())
    }

    fn write(&mut self, var: &Variable) -> io::Result<()> {
        fs::write(self.path_for(&var.key)?, DirStore::encode_file(var))
    }

    fn delete(&mut self, key: &VarKey) -> io::Result<()> {
        fs::remove_file(self.path_for(key)?)
    }
}

fn not_found(key: &VarKey) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{key} not found"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guids::EFI_GLOBAL_VARIABLE;

    fn file_name(store: &DirStore, key: &VarKey) -> String {
        store.path_for(key).unwrap().file_name().unwrap().to_str().unwrap().to_string()
    }

    #[test]
    fn file_name_round_trip() {
        let store = DirStore::new("x");
        let key = VarKey::new("Boot0001", EFI_GLOBAL_VARIABLE);
        assert_eq!(file_name(&store, &key), "Boot0001-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        assert_eq!(store.parse_file_name(&file_name(&store, &key)), Some(key));
        assert_eq!(store.parse_file_name("short"), None);
    }

    #[test]
    fn escapes_unportable_names() {
        let store = DirStore::new("x");
        for (name, escaped) in
            [("prev-lang:kbd", "prev-lang%3Akbd"), ("a%b", "a%25b"), ("..", "%2E%2E"), ("x/y\\z", "x%2Fy%5Cz"), ("t ", "t%20")]
        {
            let key = VarKey::new(name, EFI_GLOBAL_VARIABLE);
            let file = file_name(&store, &key);
            assert!(file.starts_with(escaped), "{file}");
            assert_eq!(store.parse_file_name(&file), Some(key));
        }
        // efivarfs keeps names verbatim but still rejects path traversal.
        let raw = DirStore::raw("x");
        assert!(file_name(&raw, &VarKey::new("prev-lang:kbd", EFI_GLOBAL_VARIABLE)).starts_with("prev-lang:kbd-"));
        assert!(raw.path_for(&VarKey::new("../x", EFI_GLOBAL_VARIABLE)).is_err());
    }

    #[test]
    fn dir_store_round_trip() {
        let dir = std::env::temp_dir().join(format!("efivars-core-test-{}", std::process::id()));
        let var = Variable { key: VarKey::new("Timeout", EFI_GLOBAL_VARIABLE), attributes: 7, data: vec![5, 0] };
        DirStore::dump(&dir, std::slice::from_ref(&var)).unwrap();
        let mut store = DirStore::new(&dir);
        assert_eq!(store.list().unwrap(), vec![var.clone()]);
        store.delete(&var.key).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
