//! Platform-independent UEFI variable model and parsers.
//!
//! Raw variable bytes come from a [`VarStore`] backend (Windows NT API, Linux
//! efivarfs, or an on-disk dump). [`decode::decode`] turns known variables into
//! a tree of labelled [`decode::Field`]s for display, and the typed parsers in
//! [`load_option`], [`siglist`] and friends can re-encode edited values.

pub mod attrs;
pub mod decode;
pub mod devpath;
pub mod guids;
pub mod hex;
pub mod key_option;
pub mod load_option;
pub mod siglist;
pub mod store;

pub use uguid::{Guid, guid};

/// Identifies a variable: its name plus vendor namespace GUID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VarKey {
    pub name: String,
    pub vendor: Guid,
}

impl VarKey {
    pub fn new(name: impl Into<String>, vendor: Guid) -> Self {
        Self { name: name.into(), vendor }
    }
}

impl std::fmt::Display for VarKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.name, self.vendor)
    }
}

/// A UEFI variable: key, attribute bits and raw value bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    pub key: VarKey,
    pub attributes: u32,
    pub data: Vec<u8>,
}

pub use store::VarStore;
