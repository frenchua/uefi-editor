# uefi-edit

A cross-platform terminal tool for viewing and editing UEFI variables, written
in Rust with a [Ratatui](https://ratatui.rs) interface.

Variables defined by the UEFI 2.11 specification are decoded into readable
form, and writable ones can be edited as typed fields rather than raw hex:

- Boot options as checkboxes, text and device paths
- `BootOrder` as a reorderable list
- `Timeout` as a number
- `PlatformLang` as a choice of supported languages
- `OsIndications` as named flags
- `ConIn`/`ConOut`/`ErrOut` as UEFI device-path text such as
  `PciRoot(0x0)/Pci(0x14,0x0)/USB(0x3,0x0)`

Vendor-specific variables can be viewed and hex-edited.

> **Warning:** writing bad data to boot variables can leave a machine unable
> to boot, and some firmware misbehaves on unexpected writes. The tool is
> read-only unless started with `--write`, asks before every write, and backs
> up the old value first. Even so, try edits in a virtual machine (e.g. QEMU
> with OVMF) before using them on real hardware.

## Platforms

| OS | Backend | Status |
|---|---|---|
| Windows | `NtEnumerateSystemEnvironmentValuesEx` + `Get/SetFirmwareEnvironmentVariableExW` | Reading verified on real firmware |
| Linux | efivarfs (`/sys/firmware/efi/efivars`) | Builds; not yet tested on hardware |
| Offline | A directory of dumped variables (`--dir`) | Works everywhere |

macOS and a GUI front end are planned (see [TODO.md](TODO.md)).

## Building

Requires a recent stable Rust toolchain.

```sh
cargo build --release
```

The binary is `target/release/efivars-tui` (`.exe` on Windows).

## Usage

```
efivars-tui [--dir <path>] [--write] [--list | --dump <path>]

  --dir <path>   use a directory of efivarfs-format files instead of firmware
  --write        allow edits (otherwise read-only)
  --list         print every variable, decoded, and exit
  --dump <path>  save every variable to <path> in efivarfs format and exit
```

Reading firmware variables needs elevated rights: an Administrator terminal on
Windows (for `SeSystemEnvironmentPrivilege`), or root on Linux.

Typical workflow:

```sh
efivars-tui --dump backup-dir          # snapshot everything first
efivars-tui --dir backup-dir --write   # practise edits on the copy
efivars-tui --write                    # edit the real firmware
```

### Keys

| Where | Keys |
|---|---|
| Variable list | `↑↓`/`jk` move · `/` filter · `Tab` decoded/hex view · `PgUp`/`PgDn` scroll · `e` edit · `x` hex-edit · `a` toggle a boot option's ACTIVE flag · `D` dump all · `r` reload · `q` quit |
| Form editor | `↑↓` select · `Enter`/`Space` edit or toggle · `←→` change a choice · `d` delete list item · `K`/`J` move item · `t` text/hex view of data · `s` save · `Esc` cancel |
| Device-path editor | `↑↓` select · `Enter` edit node · `a` add node · `i` add instance separator · `d` delete · `K`/`J` move · `s` save · `Esc` cancel |

Every save shows a confirmation dialog listing the decoded lines that change.
The previous value is written to `efivars-backups/<timestamp>/` (openable with
`--dir`), and the new value is read back to verify it.

## What is supported

- **Editable:**
  - `Boot####`, `Driver####`, `SysPrep####`
  - `BootOrder`, `DriverOrder`, `SysPrepOrder`, `BootNext`
  - `Timeout`, `PlatformLang`, `OsIndications`, `Key####`
  - `ConIn`, `ConOut`, `ErrOut`
  - `AuditMode`, `DeployedMode`
- **View only, because the spec says firmware owns them:**
  - `BootCurrent`, `SecureBoot`, `SetupMode`, `VendorKeys`
  - `ConInDev`, `ConOutDev`, `ErrOutDev`
  - `*Default` key stores, `OsIndicationsSupported`, `PlatformLangCodes`
  - `SignatureSupport`, `BootOptionSupport`, `HwErrRecSupport`
  - `PlatformRecovery####`, `devAuthBoot`
  - hardware error records (`HwErrRec####`)
  - capsule results (`Capsule####`, `CapsuleMax`, `CapsuleLast`)
- **View only, Secure Boot:**
  - `PK`, `KEK`, `db`, `dbx`, `dbt`, `dbr`
  - `devdb`, `OsRecoveryOrder`, `OsRecovery####`

  These are decoded to certificates and hashes. Editing them needs signed
  updates, which are not implemented yet (see [TODO.md](TODO.md)).

Creating and deleting variables is planned.

## Project layout

```
crates/
  efivars-core/   platform-independent parsing, decoding, forms, spec rules
  efivars-sys/    Windows and Linux firmware backends
  efivars-tui/    the Ratatui application
winefivars/       original C++ proof of concept (Windows enumeration)
docs/             design documentation
```

See [docs/Architecture.md](docs/Architecture.md) for the design and
[AGENTS.md](AGENTS.md) for contributor and coding-agent guidelines.

## Testing

```sh
cargo test --workspace
```

To also check every variable from a real machine, dump it and point the tests
at the dump:

```sh
efivars-tui --dump my-dump                 # elevated
EFIVARS_DUMP=my-dump cargo test --workspace
```

Dumps contain machine-specific data (disk GUIDs, certificates) and should not
be committed.
