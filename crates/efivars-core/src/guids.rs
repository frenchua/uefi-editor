//! Well-known vendor and signature-type GUIDs.

use uguid::{Guid, guid};

pub const EFI_GLOBAL_VARIABLE: Guid = guid!("8be4df61-93ca-11d2-aa0d-00e098032b8c");
pub const EFI_IMAGE_SECURITY_DATABASE: Guid = guid!("d719b2cb-3d3a-4596-a3bc-dad00e67656f");
pub const MICROSOFT_VENDOR: Guid = guid!("77fa9abd-0359-4d32-bd60-28f4e78f784b");
pub const SHIM_LOCK: Guid = guid!("605dab50-e046-4300-abb6-3dd810dd8b23");

pub const EFI_CERT_SHA1: Guid = guid!("826ca512-cf10-4ac9-b187-be01496631bd");
pub const EFI_CERT_SHA256: Guid = guid!("c1c41626-504c-4092-aca9-41f936934328");
pub const EFI_CERT_SHA384: Guid = guid!("ff3e5307-9fd0-48c9-85f1-8ad56c701e01");
pub const EFI_CERT_SHA512: Guid = guid!("093e0fae-a6c4-4f50-9f1b-d41e2b89c19a");
pub const EFI_CERT_RSA2048: Guid = guid!("3c5766e8-269c-4e34-aa14-ed776e85b3b6");
pub const EFI_CERT_X509: Guid = guid!("a5c059a1-94e4-4aa7-87b5-ab155c2bf072");
pub const EFI_CERT_X509_SHA256: Guid = guid!("3bd2a492-96c0-4079-b420-fcf98ef103ed");
pub const EFI_CERT_X509_SHA384: Guid = guid!("7076876e-80c2-4ee6-aad2-28b349a6865b");
pub const EFI_CERT_X509_SHA512: Guid = guid!("446dbf63-2502-4cda-bcfa-2465d2b0fe9d");

/// Human-friendly name for a vendor namespace GUID, if known.
pub fn vendor_name(guid: &Guid) -> Option<&'static str> {
    Some(match *guid {
        EFI_GLOBAL_VARIABLE => "EFI Global",
        EFI_IMAGE_SECURITY_DATABASE => "Image Security DB",
        MICROSOFT_VENDOR => "Microsoft",
        SHIM_LOCK => "Shim",
        _ => return None,
    })
}

/// Human-friendly name for an `EFI_SIGNATURE_LIST` type GUID.
pub fn cert_type_name(guid: &Guid) -> Option<&'static str> {
    Some(match *guid {
        EFI_CERT_SHA1 => "SHA1",
        EFI_CERT_SHA256 => "SHA256",
        EFI_CERT_SHA384 => "SHA384",
        EFI_CERT_SHA512 => "SHA512",
        EFI_CERT_RSA2048 => "RSA2048",
        EFI_CERT_X509 => "X509",
        EFI_CERT_X509_SHA256 => "X509_SHA256",
        EFI_CERT_X509_SHA384 => "X509_SHA384",
        EFI_CERT_X509_SHA512 => "X509_SHA512",
        _ => return None,
    })
}
