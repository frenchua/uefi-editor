//! Linux backend over efivarfs (`/sys/firmware/efi/efivars`).
//!
//! Files are `<Name>-<guid>` holding a 4-byte attribute word plus the value
//! (the same layout as [`efivars_core::store::DirStore`]). The kernel marks
//! most files immutable, so writes and deletes clear `FS_IMMUTABLE_FL` first,
//! and every write must be a single `write(2)` of attributes + data.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use efivars_core::store::DirStore;
use efivars_core::{VarKey, VarStore, Variable};
use rustix::fs::{IFlags, ioctl_getflags, ioctl_setflags};

pub const EFIVARFS_PATH: &str = "/sys/firmware/efi/efivars";

pub struct EfivarfsStore {
    dir: DirStore,
}

impl EfivarfsStore {
    pub fn new() -> io::Result<Self> {
        Self::at(EFIVARFS_PATH)
    }

    pub fn at(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} not found: not booted via UEFI, or efivarfs is not mounted", root.display()),
            ));
        }
        Ok(Self { dir: DirStore::raw(root) })
    }
}

fn clear_immutable(path: &Path) -> io::Result<()> {
    let file = File::open(path)?;
    let flags = ioctl_getflags(&file)?;
    if flags.contains(IFlags::IMMUTABLE) {
        ioctl_setflags(&file, flags - IFlags::IMMUTABLE)?;
    }
    Ok(())
}

impl VarStore for EfivarfsStore {
    fn describe(&self) -> String {
        format!("efivarfs {}", self.dir.root.display())
    }

    fn list(&mut self) -> io::Result<Vec<Variable>> {
        self.dir.list()
    }

    fn read(&mut self, key: &VarKey) -> io::Result<Variable> {
        self.dir.read(key)
    }

    fn write(&mut self, var: &Variable) -> io::Result<()> {
        let path = self.dir.path_for(&var.key)?;
        if path.exists() {
            clear_immutable(&path)?;
        }
        let buf = DirStore::encode_file(var);
        let mut file = OpenOptions::new().write(true).create(true).open(&path)?;
        let written = file.write(&buf)?;
        if written != buf.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short write: {written} of {} bytes", buf.len())));
        }
        Ok(())
    }

    fn delete(&mut self, key: &VarKey) -> io::Result<()> {
        let path = self.dir.path_for(key)?;
        clear_immutable(&path)?;
        fs::remove_file(path)
    }
}
