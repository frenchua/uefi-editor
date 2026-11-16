# AGENTS.md

Guidance for AI coding agents and contributors working in this repository.
Read [docs/Architecture.md](docs/Architecture.md) first; it explains the crate
layout, the invariants below, and the project history.

## Commands

```sh
cargo build                                    # whole workspace
cargo test --workspace                         # all tests (no firmware access needed)
cargo clippy --workspace --all-targets
cargo check -p efivars-sys --target x86_64-unknown-linux-gnu   # type-check Linux backend from Windows
cargo run -p efivars-tui -- --dir <dump> --write                # try edits against a dump, not firmware
```

Optional real-data tests: `EFIVARS_DUMP=<dir> cargo test --workspace`, where
`<dir>` was produced by `efivars-tui --dump <dir>` on an elevated terminal.

`cargo test` does not rebuild `target/debug/efivars-tui`. Run `cargo build`
before running the binary by hand.

## Safety rules

This tool writes to firmware. Mistakes can make a machine unbootable.

- **Never write to live firmware** from tests, scripts, or experiments. Use
  `MemStore`, `DirStore` (`--dir`), or a VM with OVMF. Tests must never call
  `efivars_sys::open_system()`.
- **Never commit firmware dumps.** They contain disk GUIDs, certificates and
  other machine-specific data (`efivars-dump-*/` and `efivars-backups/` are
  gitignored).
- Tests that exercise the write path create `efivars-backups/` in the crate
  directory. Remove it at the end of the test, as the existing tests do.

## Invariants to preserve

1. **Exact round-trip.** A structured editor may only open a variable whose
   bytes re-encode identically through its model. Device paths use
   `DevicePath::parse_exact`/`parse_list`; forms require
   `form::encode(build(x)) == x`. Otherwise fall back to the hex editor with a
   reason.
2. **Lossless edit text.** `Node::edit_text` must always parse back to the
   identical node. New node renderings need a `from_text` arm and an entry in
   the `text_forms_round_trip` test.
3. **Preserve what the user didn't touch.** Undefined attribute bits,
   optional data, and unknown flag bits must survive edits.
4. **Spec scope.** Only variables defined by the latest UEFI specification
   (currently 2.11) are decoded or given forms. Do not add parsers for
   vendor-specific variables (ASUS `Setup`, Microsoft, Apple, etc.).
5. **Secure Boot variables are view-only** (`PK`, `KEK`, `db`, `dbx`, `dbt`,
   `dbr`, `devdb`, `OsRecoveryOrder`, `OsRecovery####`). Don't enable editing
   them without implementing signed `EFI_VARIABLE_AUTHENTICATION_2` updates
   (see `TODO.md`).
6. **Spec access rules** live in `crates/efivars-core/src/spec.rs`. Every
   editor, including hex, must respect them.
7. **No format knowledge in the UI.** Byte layouts, decoding and validation
   belong in `efivars-core`. `efivars-tui` only renders `decode::Field` trees
   and edits `form::Value`s, so a future GUI can reuse the core.

## Adding support for a spec variable

1. Confirm the variable, its namespace GUID, its attributes and its layout in
   the current UEFI spec. Don't rely on memory: constants and sub-type numbers
   have been wrong before. Cite the section in a doc comment.
2. Add GUIDs to `guids.rs`, and the structure to a module with a binrw
   definition if it is written back.
3. Add a `Kind` and `classify` arm in `decode.rs`, plus a decoder producing
   `Field`s.
4. Set its access in `spec.rs`.
5. If it is writable, add `build`/`encode` arms in `form.rs` (and to
   `has_form`), reusing existing `Value` types where possible.
6. Tests: a unit test for the parser and decoder, an unchanged form
   round-trip (`every_form_round_trips_unchanged`), and an edit test.

## Style

- Rust 2024 edition; `cargo fmt` defaults with long lines tolerated.
- Error messages are for end users: say what is wrong and what valid input
  looks like (e.g. "USB takes 2 argument(s), got 1").
- Prefer bounds-checked parsing (`get`, `checked_*`) over indexing on
  untrusted firmware data; parsers must never panic on malformed bytes.
- Keep `README.md`, `docs/Architecture.md` and `TODO.md` in sync with
  behaviour changes.

## Platform notes

- Windows: firmware access needs an elevated process
  (`SeSystemEnvironmentPrivilege`). Without it, `--list` fails with OS error
  1300; use `--dir` with an existing dump instead.
- Linux: efivarfs files are immutable by default; the backend clears
  `FS_IMMUTABLE_FL` before writes. It has not been tested on hardware.
- `winefivars/` is the original C++ proof of concept for Windows enumeration.
  It is kept for reference and is not part of the Cargo build.
