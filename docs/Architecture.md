# Architecture

This document describes how uefi-edit is designed, the invariants the code
relies on, and how the project got to its current state.

## Goals

- Read every UEFI variable on Windows and Linux (macOS later) through one
  interface.
- Decode the variables defined by the **UEFI 2.11 specification** into a
  readable form. Vendor-specific variables are never interpreted.
- Let the user edit the writable spec variables as typed fields, not raw hex,
  without ever silently changing bytes they did not touch.
- Keep the UI swappable: a GUI is planned, so all format knowledge lives
  outside the TUI.

## Crate structure

```
efivars-core   (no OS dependencies)
   ▲      ▲
   │      └──────────── efivars-tui (Ratatui app, binary)
   │                        │
efivars-sys ◄───────────────┘
(Windows / Linux backends)
```

| Crate | Responsibility | Key dependencies |
|---|---|---|
| `efivars-core` | Variable model, storage trait, binary parsers, decoders, editable forms, spec access rules | `binrw`, `uguid`, `x509-parser` |
| `efivars-sys` | `VarStore` implementations against live firmware | `windows-sys` (Windows), `rustix` (Linux) |
| `efivars-tui` | Terminal UI: browsing, editors, confirm/backup/write flow | `ratatui` (re-exports crossterm) |

A future GUI would depend on `efivars-core` and `efivars-sys` exactly as the
TUI does.

## Data model and storage

`efivars_core::Variable` is `{ key: VarKey { name, vendor: Guid }, attributes: u32, data: Vec<u8> }`.
Everything that reads or writes variables implements `VarStore`
(`crates/efivars-core/src/store.rs`):

```rust
trait VarStore {
    fn describe(&self) -> String;
    fn list(&mut self) -> io::Result<Vec<Variable>>;
    fn read(&mut self, key: &VarKey) -> io::Result<Variable>;
    fn write(&mut self, var: &Variable) -> io::Result<()>;
    fn delete(&mut self, key: &VarKey) -> io::Result<()>;
}
```

Implementations:

| Store | Where | Notes |
|---|---|---|
| `WindowsStore` | `efivars-sys/src/windows.rs` | Enables `SeSystemEnvironmentPrivilege`. Enumerates with `NtEnumerateSystemEnvironmentValuesEx` (class 2, `VARIABLE_NAME_AND_VALUE` records, a bounds-checked port of `winefivars/winefivars.cpp`). Reads and writes single variables with `Get/SetFirmwareEnvironmentVariableExW`. |
| `EfivarfsStore` | `efivars-sys/src/linux.rs` | Files are `<Name>-<guid>` holding 4 attribute bytes plus the data. Clears `FS_IMMUTABLE_FL` before writing or deleting, and writes attributes and data in a single `write(2)`. |
| `DirStore` | `efivars-core/src/store.rs` | Same file format as efivarfs, for dumps (`--dump`, `--dir`) and backups. Portable mode (`DirStore::new`) escapes characters that file systems reject as `%XX`. Raw mode (`DirStore::raw`, used by efivarfs) keeps names verbatim. |
| `MemStore` | `efivars-core/src/store.rs` | In-memory, for tests. |

`efivars_sys::open_system()` picks the backend for the current OS.

## Parsing layer

Spec structures that are written back are defined once with **binrw**
`#[binrw]` derives, so a single definition yields both the reader and the
writer. Length and count fields are `#[br(temp)] #[bw(calc = …)]`, so they are
recomputed from the data on write and can never disagree with it.

| Module | Structure | Spec |
|---|---|---|
| `load_option.rs` | `EFI_LOAD_OPTION` (Boot/Driver/SysPrep/PlatformRecovery/OsRecovery####) | §3.1.3 |
| `key_option.rs` | `EFI_KEY_OPTION`, `EFI_INPUT_KEY`, CRC-32 (`CalculateCrc32`) | §3.1.6 |
| `siglist.rs` | `EFI_SIGNATURE_LIST` / `EFI_SIGNATURE_DATA` databases | §32.4.1 |
| `devpath.rs` | `EFI_DEVICE_PATH_PROTOCOL`: binary ⇄ nodes ⇄ text | §10.3, §10.6 |
| `cper.rs` | Common Platform Error Record (display only) | Appendix N |
| `capsule.rs` | `EFI_CAPSULE_RESULT_VARIABLE_*`, `EFI_TIME`, `EFI_STATUS` (display only) | §8.5.6, App. D |

### Device paths

Device paths are kept as raw node bytes and interpreted on demand:

- `DevicePath::parse` reads nodes. End-Instance starts a new instance;
  End-Entire terminates the path.
- `DevicePath::parse_list` splits a packed array of End-Entire-terminated
  paths (a load option's `FilePathList`).
- `DevicePath::parse_exact` succeeds only if re-serializing gives exactly the
  input bytes. This guards every structured editor.
- `Node::to_text` is the friendly display form. `Node::edit_text` is
  guaranteed to parse back to the identical node: it uses the friendly form
  when that round-trips, and the generic `Path(type,subtype,hex)` form
  otherwise (e.g. IPv4 nodes whose display is abbreviated).
- `Node::from_text` parses the §10.6 text forms (PCI, ACPI, USB, SATA, NVMe,
  SCSI, MAC, URI, SD, eMMC, Wi-Fi, HD, CDROM, file paths, vendor nodes, and
  others), with range-checked numbers and specific error messages.

## Decoding layer: what a variable *is*

`decode.rs`:

- `classify(vendor, name) -> Option<Kind>` maps spec-defined
  `(namespace GUID, name)` pairs to a structure kind. It only recognizes names
  in the UEFI 2.11 namespaces:
  - `EFI_GLOBAL_VARIABLE`
  - `EFI_IMAGE_SECURITY_DATABASE_GUID`
  - `EFI_DEVICE_SECURITY_DATABASE_GUID`
  - `EFI_HARDWARE_ERROR_VARIABLE`
  - `EFI_CAPSULE_REPORT_GUID`
  - `OsRecovery####`, which lives in OS vendor namespaces.

  Everything else is `None` and shown as hex.
- `decode(var)` produces a `Vec<Field>` display tree (`label`, `value`,
  `children`), used by the decoded view, `--list`, and the change summary in
  the confirmation dialog.

## Spec access rules

`spec.rs` encodes what the spec says about who may write each variable,
independent of the attribute bits the firmware reports:

- `Access::Editable`: the OS may write it.
- `Access::ReadOnly(reason)`: firmware-owned, e.g. `BootCurrent`,
  `SecureBoot`, `*Default`, `OsIndicationsSupported`, `PlatformRecovery####`,
  `HwErrRec####`, capsule results.
- `Access::SecureBoot`: time-based authenticated (`PK`, `KEK`, `db`, `dbx`,
  `dbt`, `dbr`, `devdb`, `OsRecoveryOrder`, `OsRecovery####`). Writes need an
  `EFI_VARIABLE_AUTHENTICATION_2` signed payload, which is not implemented
  (`TODO.md`).

The TUI checks both this table and the authenticated attribute bits before
opening any editor, including the hex editor.

## Editing layer: forms

`form.rs` turns a variable into typed, editable fields so the UI does not need
to know any variable layout:

```rust
enum Value {
    Uint { value, bits }, Bool, Flags { value, defs }, Choice { value, options },
    Text { value, ascii }, Data { bytes, view: Hex | Ucs2 }, Guid,
    DevicePath, OptionRef { number, prefix }, Key(InputKey),
    List { items, template, max },
}
```

- `form::build(kind, name, data, vars)` produces a `Vec<Field>`; `vars`
  supplies context such as `PlatformLangCodes`, `OsIndicationsSupported`, and
  the `Boot####` entry a key option points to.
- `form::encode(kind, fields, vars)` produces the new bytes.
- Each `Value` knows how to display itself, parse typed text with validation
  (`set_text`), toggle, or cycle.

Per-variable decisions:

| Variable | Form |
|---|---|
| `Boot####` etc. | ACTIVE/FORCE_RECONNECT/HIDDEN flags (undefined bits preserved), Boot/App category, description, list of device paths (≥ 1 required), optional data (shown as UCS-2 text when it is clean printable text, such as a kernel command line, otherwise hex) |
| `*Order` | List of option references; duplicates rejected; prefix follows the variable (`DriverOrder` lists `Driver####`) |
| `BootNext` | Option reference |
| `Timeout` | 16-bit number |
| `PlatformLang` | Choice from `PlatformLangCodes`; written NUL-terminated |
| `OsIndications` | Named flags; bits the firmware does not support are annotated |
| `Key####` | Modifier flags, target option, up to 3 keys; `BootOptionCrc` and the key count are recomputed on save |
| `AuditMode`, `DeployedMode` | Boolean |
| `ConIn`/`ConOut`/`ErrOut` | Not a form; edited in the dedicated device-path editor |

## TUI

`efivars-tui` is a mode machine (`app.rs`):

```
Normal ─/─► Filter
   │
   ├─e─► FormEdit ──(device path field)──► embedded PathEditor
   ├─e─► PathEdit       (ConIn/ConOut/ErrOut)
   ├─x─► HexEdit        (any writable variable)
   │
   └── every editor ─s─► Confirm ─y─► backup → write → read back → reload
```

- `form_editor.rs` is a generic editor over `form::Field`s. It flattens fields
  into rows (field, flag checkbox, list item, "add" row), and embeds
  `PathEditor` for device-path values.
- `path_editor.rs` presents a device path as nodes plus instance separators,
  each node edited as §10.6 text.
- `line_input.rs` is the shared single-line text input.
- `ui.rs` renders everything; `app.rs` owns state, key handling, and the write
  flow.

Write safety, in order:

1. The process is read-only unless started with `--write`.
2. The spec access table and the authenticated attribute bits are checked.
3. A structured editor opens only if the variable's bytes re-encode exactly
   through that editor's model (`parse_exact`, and `encode(build(x)) == x` for
   forms). Otherwise the editor falls back to hex and says why.
4. A save that produces identical bytes is reported as "no changes".
5. The confirmation dialog lists the decoded lines that change.
6. The current stored value is backed up to `efivars-backups/<ms>/` in
   `DirStore` format before writing, and the write is aborted if the backup
   fails.
7. The value is read back and compared after writing.

`APPEND_WRITE` is masked off before writing, since it is a write-time flag,
not a stored attribute.

## Testing strategy

- **Unit tests** in every core module: binrw round-trips, device-path text
  forms (every supported form must render and parse back identically), error
  messages, CPER and capsule decoding, spec access rules, and form build and
  encode for each kind.
- **Real-data test**, `crates/efivars-core/tests/dump_round_trip.rs`. Given
  `EFIVARS_DUMP=<dir>` it checks that:
  - every spec variable decodes
  - every structured variable re-encodes byte-for-byte
  - every device path survives bytes → edit text → bytes
  - every form survives build → encode unchanged.

  It is skipped without the environment variable.
- **TUI tests** in `app.rs` drive the app with key events against a `MemStore`
  and render with Ratatui's `TestBackend`. They cover the form, path and hex
  editors, spec refusals, and the confirm/backup/write path. One more test
  opens every writable variable of a real dump in its editor.
- `cargo check -p efivars-sys --target x86_64-unknown-linux-gnu` type-checks
  the Linux backend from Windows.

## History

1. **Library choice.** The question was how to parse C-style UEFI structs
   cross-platform. The answer was binrw, which is declarative and
   bidirectional, so edits round-trip, plus `uguid` for GUIDs and
   `x509-parser` for Secure Boot certificates. A plain `#[repr(C)]` cast was
   ruled out because most interesting variables are variable-length.
2. **Workspace.** The work created the three crates and the
   `VarStore` abstraction. The Windows backend was ported from the C++ proof
   of concept in `winefivars/`; the Linux backend uses efivarfs. The first
   TUI had list, decoded and hex views, a hex editor, a `BootOrder` reorder
   editor, and a boot-option ACTIVE toggle. Tests caught an arithmetic
   overflow on malformed signature lists: binrw evaluates `count` before
   `assert`, so the check moved to an earlier field.
3. **Real firmware (Windows, elevated).**
   - All 140 variables on an ASUS machine dual-booting Windows and Gentoo
     decoded without errors, and the boot entries matched
     `bcdedit /enum firmware`.
   - Dumps exposed a bug: the variable `prev-lang:kbd` contains `:`, which
     NTFS treats as an alternate data stream, so the variable was silently
     lost from the dump. This led to portable `%XX` escaping in `DirStore`.
4. **Device-path editor.**
   - Added text → binary parsing (`Node::from_text`), the lossless
     `edit_text`, `to_bytes`, and the node-list editor for
     `ConIn`/`ConOut`/`ErrOut`.
   - This fixed wrong Messaging sub-type numbers: SD is 26, Wi-Fi 28 and
     eMMC 29, previously mislabelled as 31, 30 and 22.
   - Every real device path (8 paths, 60 nodes) round-trips through text.
5. **Editing everything in the spec.**
   - Checking the UEFI 2.11 text set the scope:
     - `Lang` and `LangCodes` are no longer in the global variable table.
     - `CryptoIndications` is not in 2.11.
     - `devdb` has its own namespace.
     - `HwErrRec####` and capsule results live in their own namespaces.
     - Many globals are firmware-only.
   - Added the spec access table, the generic form model and editor
     (replacing the special-purpose `BootOrder` editor), multi-path load
     options, `Key####` editing with CRC recomputation, and CPER and capsule
     decoders.
   - Secure Boot variables were kept view-only by decision, recorded in
     `TODO.md`.

## Known limitations

- Writes to real firmware have only been exercised against in-memory and
  directory stores in tests, not against live firmware.
- The Linux backend compiles but has not run on hardware.
- Secure Boot variable editing (signed updates) is not implemented.
- Creating and deleting variables is not implemented.
- `EFI_FIRMWARE_MANAGEMENT_CAPSULE_ID_GUID` (used to decode FMP capsule
  results in detail) was taken from memory, not confirmed against the spec
  text. A wrong value only means those results are shown as hex.
- Long values wrap mid-word in narrow terminals.
