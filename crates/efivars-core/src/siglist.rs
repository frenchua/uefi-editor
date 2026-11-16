//! `EFI_SIGNATURE_LIST` (UEFI spec 32.4.1): the payload of PK, KEK, db, dbx…

use std::io::Cursor;

use binrw::helpers::until_eof;
use binrw::{BinRead, BinWrite, binrw};

use crate::Guid;

/// Size of the fixed `EFI_SIGNATURE_LIST` header.
pub const HEADER_LEN: u32 = 16 + 4 + 4 + 4;

/// A whole signature database: a back-to-back sequence of lists.
#[binrw]
#[brw(little)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureDatabase {
    #[br(parse_with = until_eof)]
    pub lists: Vec<SignatureList>,
}

#[binrw]
#[brw(little)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureList {
    #[br(map = Guid::from_bytes)]
    #[bw(map = |g: &Guid| g.to_bytes())]
    pub signature_type: Guid,
    #[br(temp)]
    #[bw(try_calc = u32::try_from(HEADER_LEN as usize + header.len() + signatures.len() * *signature_size as usize))]
    list_size: u32,
    #[br(temp)]
    #[bw(try_calc = u32::try_from(header.len()))]
    header_size: u32,
    #[br(assert(signature_size >= 16, "SignatureSize {} smaller than owner GUID", signature_size))]
    pub signature_size: u32,
    // Validated here, before `signatures` computes its count from the sizes.
    #[br(
        assert(list_size >= HEADER_LEN.saturating_add(header_size), "SignatureListSize {} too small", list_size),
        count = header_size
    )]
    pub header: Vec<u8>,
    #[br(
        count = (list_size - HEADER_LEN - header_size) / signature_size,
        args { inner: (signature_size,) }
    )]
    #[bw(args(*signature_size))]
    pub signatures: Vec<SignatureData>,
}

#[binrw]
#[brw(little)]
#[br(import(signature_size: u32))]
#[bw(import(signature_size: u32))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureData {
    #[br(map = Guid::from_bytes)]
    #[bw(map = |g: &Guid| g.to_bytes())]
    pub owner: Guid,
    #[br(count = signature_size - 16)]
    #[bw(assert(data.len() as u32 == signature_size - 16, "signature data length must equal SignatureSize - 16"))]
    pub data: Vec<u8>,
}

impl SignatureDatabase {
    pub fn parse(bytes: &[u8]) -> binrw::BinResult<Self> {
        Self::read(&mut Cursor::new(bytes))
    }

    pub fn to_bytes(&self) -> binrw::BinResult<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        self.write(&mut out)?;
        Ok(out.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guids::{EFI_CERT_SHA256, MICROSOFT_VENDOR};

    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&EFI_CERT_SHA256.to_bytes());
        b.extend_from_slice(&(HEADER_LEN + 2 * 48).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&48u32.to_le_bytes());
        for fill in [0xaa, 0xbb] {
            b.extend_from_slice(&MICROSOFT_VENDOR.to_bytes());
            b.extend_from_slice(&[fill; 32]);
        }
        b
    }

    #[test]
    fn round_trip() {
        let mut bytes = sample();
        bytes.extend(sample());
        let db = SignatureDatabase::parse(&bytes).unwrap();
        assert_eq!(db.lists.len(), 2);
        assert_eq!(db.lists[0].signature_type, EFI_CERT_SHA256);
        assert_eq!(db.lists[0].signatures[1].owner, MICROSOFT_VENDOR);
        assert_eq!(db.lists[0].signatures[1].data, vec![0xbb; 32]);
        assert_eq!(db.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn removing_a_signature_recomputes_size() {
        let mut db = SignatureDatabase::parse(&sample()).unwrap();
        db.lists[0].signatures.pop();
        let bytes = db.to_bytes().unwrap();
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), HEADER_LEN + 48);
        assert_eq!(SignatureDatabase::parse(&bytes).unwrap(), db);
    }

    #[test]
    fn rejects_bad_sizes() {
        let mut bytes = sample();
        bytes[16..20].copy_from_slice(&4u32.to_le_bytes());
        assert!(SignatureDatabase::parse(&bytes).is_err());
    }
}
