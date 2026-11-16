//! Round-trips every structured variable in a real dump produced by
//! `efivars-tui --dump <dir>`. Dumps hold machine-specific data (disk GUIDs,
//! certificates), so they are not committed; point the test at one with
//! `EFIVARS_DUMP=<dir> cargo test -p efivars-core --test dump_round_trip`.
//! Without the variable the test passes trivially.

use efivars_core::decode::{Kind, classify, decode};
use efivars_core::form;
use efivars_core::devpath::{DevicePath, Node};
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

    // Every device path (standalone or inside a load option) must survive
    // bytes -> per-node edit text -> bytes, which is what the TUI editor does.
    let mut paths = 0;
    let mut nodes = 0;
    for v in &vars {
        let bytes = match classify(&v.key.vendor, &v.key.name) {
            Some(Kind::DevicePath) => v.data.clone(),
            Some(Kind::LoadOption) => LoadOption::parse(&v.data).unwrap().file_path_list,
            _ => continue,
        };
        let Ok(path) = DevicePath::parse_exact(&bytes) else {
            eprintln!("{}: not canonical, editor would fall back to hex", v.key);
            continue;
        };
        let rebuilt = DevicePath {
            instances: path
                .instances
                .iter()
                .map(|inst| {
                    inst.iter()
                        .map(|n| {
                            nodes += 1;
                            let text = n.edit_text();
                            Node::from_text(&text).unwrap_or_else(|e| panic!("{}: {text:?}: {e}", v.key))
                        })
                        .collect()
                })
                .collect(),
        };
        assert_eq!(rebuilt.to_bytes().unwrap(), bytes, "{} device path did not round-trip via text", v.key);
        paths += 1;
    }
    eprintln!("{paths} device paths ({nodes} nodes) round-tripped through edit text");

    // Every variable with an editable form must re-encode unchanged when the
    // form is saved without edits.
    let mut forms = Vec::new();
    for v in &vars {
        let Some(kind) = classify(&v.key.vendor, &v.key.name).filter(|k| form::has_form(*k)) else { continue };
        let fields = form::build(kind, &v.key.name, &v.data, &vars).unwrap_or_else(|e| panic!("{}: {e}", v.key));
        assert_eq!(form::encode(kind, &fields, &vars).unwrap(), v.data, "{} form did not round-trip", v.key);
        forms.push(v.key.name.clone());
    }
    eprintln!("{} forms round-tripped: {}", forms.len(), forms.join(", "));
}
