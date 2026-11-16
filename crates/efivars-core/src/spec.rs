//! Write access rules from the UEFI 2.11 specification (§3.3 Table 3.1,
//! §8.2.7, §8.5.6, §32), independent of the attribute bits a firmware reports.

use crate::guids::{
    EFI_CAPSULE_REPORT, EFI_DEVICE_SECURITY_DATABASE, EFI_GLOBAL_VARIABLE, EFI_HARDWARE_ERROR_VARIABLE,
    EFI_IMAGE_SECURITY_DATABASE,
};
use crate::load_option::is_numbered;
use crate::Guid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The OS may write it (subject to the firmware's own checks).
    Editable,
    /// The spec says only firmware sets it; the reason is shown to the user.
    ReadOnly(&'static str),
    /// Secure Boot policy variable: writes need a signed authentication
    /// descriptor, which this tool does not produce yet (see TODO.md).
    SecureBoot,
}

impl Access {
    pub fn blocker(self, name: &str) -> Option<String> {
        match self {
            Access::Editable => None,
            Access::ReadOnly(why) => Some(format!("{name} is read-only per the UEFI spec: {why}")),
            Access::SecureBoot => Some(format!(
                "{name} is a Secure Boot variable; editing it needs a signed update and is not supported yet (see TODO.md)"
            )),
        }
    }
}

/// Access rule for a variable. Variables the spec does not define (vendor
/// variables) are `Editable`: they can only be hex-edited.
pub fn access(vendor: &Guid, name: &str) -> Access {
    use Access::*;
    // OS-defined recovery options live in OS vendor namespaces listed in
    // OsRecoveryOrder and are time-based authenticated (§3.4.1).
    if is_numbered(name, "OsRecovery") {
        return SecureBoot;
    }
    match *vendor {
        EFI_IMAGE_SECURITY_DATABASE => SecureBoot,
        EFI_DEVICE_SECURITY_DATABASE if name == "devdb" => SecureBoot,
        EFI_HARDWARE_ERROR_VARIABLE if is_numbered(name, "HwErrRec") => {
            ReadOnly("hardware error records are written by firmware (deleting them is not supported yet)")
        }
        EFI_CAPSULE_REPORT => ReadOnly("capsule results are written by firmware"),
        EFI_GLOBAL_VARIABLE => match name {
            "PK" | "KEK" | "OsRecoveryOrder" => SecureBoot,
            "PKDefault" | "KEKDefault" | "dbDefault" | "dbrDefault" | "dbtDefault" | "dbxDefault"
            | "devdbDefault" => ReadOnly("OEM default key stores are fixed by firmware"),
            "BootCurrent" => ReadOnly("firmware sets it to the option used for this boot"),
            "BootOptionSupport" => ReadOnly("it reports boot manager capabilities"),
            "ConInDev" | "ConOutDev" | "ErrOutDev" => ReadOnly("it lists every device firmware found"),
            "devAuthBoot" => ReadOnly("it reports device authentication mode"),
            "HwErrRecSupport" => ReadOnly("only firmware may set the error-record persistence level"),
            "OsIndicationsSupported" => ReadOnly("firmware recreates it every boot"),
            "PlatformLangCodes" => ReadOnly("it lists the languages firmware supports"),
            "SignatureSupport" => ReadOnly("it lists the signature types firmware supports"),
            "SecureBoot" | "SetupMode" | "VendorKeys" => ReadOnly("it reports Secure Boot state"),
            _ if is_numbered(name, "PlatformRecovery") => ReadOnly("platform recovery options are firmware-only"),
            _ => Editable,
        },
        _ => Editable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guids::MICROSOFT_VENDOR;

    #[test]
    fn access_rules() {
        assert_eq!(access(&EFI_GLOBAL_VARIABLE, "Boot0001"), Access::Editable);
        assert_eq!(access(&EFI_GLOBAL_VARIABLE, "BootNext"), Access::Editable);
        assert!(matches!(access(&EFI_GLOBAL_VARIABLE, "BootCurrent"), Access::ReadOnly(_)));
        assert!(matches!(access(&EFI_GLOBAL_VARIABLE, "PlatformRecovery0000"), Access::ReadOnly(_)));
        assert_eq!(access(&EFI_GLOBAL_VARIABLE, "KEK"), Access::SecureBoot);
        assert_eq!(access(&EFI_IMAGE_SECURITY_DATABASE, "dbx"), Access::SecureBoot);
        assert_eq!(access(&MICROSOFT_VENDOR, "OsRecovery0001"), Access::SecureBoot);
        assert_eq!(access(&MICROSOFT_VENDOR, "Anything"), Access::Editable);
        assert!(access(&EFI_GLOBAL_VARIABLE, "SecureBoot").blocker("SecureBoot").unwrap().contains("read-only"));
    }
}
