//! Round-trips every structured variable in a real dump produced by
//! `efivars-tui --dump <dir>`. Dumps hold machine-specific data (disk GUIDs,
//! certificates), so they are not committed; point the test at one with
//! `EFIVARS_DUMP=<dir> cargo test -p efivars-core --test dump_round_trip`.
//! Without the variable the test passes trivially.

use efivars_core::decode::{Kind, classify, decode};
use efivars_core::key_option::KeyOption;
use efivars_core::load_option::LoadOption;
use efivars_core::siglist::SignatureDatabase;
use efivars_core::store::DirStore;
use efivars_core::VarStore;

#[test]
fn real_dump_round_trips() {
    let Some(dir) = std::env::var_os("EFIVARS_DUMP") else {
        eprintln!("EFIVARS_DUMP not set; skipping");
        return;
    };
    let vars = DirStore::new(dir).list().unwrap();
    assert!(!vars.is_empty(), "dump directory is empty");

    let mut checked = 0;
    for v in &vars {
        if let Some(Err(e)) = decode(v) {
            panic!("{} failed to decode: {e}", v.key);
        }
        let reencoded = match classify(&v.key.vendor, &v.key.name) {
            Some(Kind::LoadOption) => LoadOption::parse(&v.data).unwrap().to_bytes().unwrap(),
            Some(Kind::SignatureDatabase) => SignatureDatabase::parse(&v.data).unwrap().to_bytes().unwrap(),
            Some(Kind::KeyOption) => KeyOption::parse(&v.data).unwrap().to_bytes().unwrap(),
            _ => continue,
        };
        assert_eq!(reencoded, v.data, "{} did not round-trip", v.key);
        checked += 1;
    }
    eprintln!("{checked} structured variables round-tripped out of {}", vars.len());
}
