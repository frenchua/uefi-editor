//! Platform backends implementing [`efivars_core::VarStore`] against live firmware.

use std::io;

use efivars_core::VarStore;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

/// Open the firmware variable store for the current OS.
pub fn open_system() -> io::Result<Box<dyn VarStore>> {
    #[cfg(windows)]
    {
        Ok(Box::new(windows::WindowsStore::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::EfivarfsStore::new()?))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no UEFI variable backend for this OS"))
    }
}
