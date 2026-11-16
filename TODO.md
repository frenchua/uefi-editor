# TODO

## Editing Secure Boot variables is not supported

The following variables are decoded and shown in readable form (certificates,
hashes, GUIDs), but **cannot be edited**:

| Variable | Namespace |
|---|---|
| `PK`, `KEK`, `OsRecoveryOrder` | EFI_GLOBAL_VARIABLE |
| `db`, `dbx`, `dbt`, `dbr` | EFI_IMAGE_SECURITY_DATABASE_GUID |
| `devdb` | EFI_DEVICE_SECURITY_DATABASE_GUID |
| `OsRecovery####` | OS vendor GUIDs listed in `OsRecoveryOrder` |

These are time-based authenticated variables (UEFI 2.11 §8.2.2, §32). Firmware
only accepts a write whose data is prefixed with an `EFI_VARIABLE_AUTHENTICATION_2`
descriptor, signed by the key one level up (PK signs PK and KEK; KEK signs db,
dbx, dbt, dbr). The editor refuses both structured and hex edits of them
(`crates/efivars-core/src/spec.rs`).

Supporting this would need:

- Editing signature lists: add or remove X.509 certificates and hashes.
- Building the signed update: PKCS#7 `SignedData` over name, vendor GUID,
  attributes, timestamp and data, using a user-supplied key and certificate.
- `APPEND_WRITE` support for adding db/dbx entries without replacing the list.
- Exporting the signed `.auth` payload for other tools (KeyTool, efi-updatevar).

## Planned

- Create and delete variables: a new `Boot####` entry, setting `BootNext`,
  removing stale boot entries and `HwErrRec####` records.
- macOS backend and a GUI front end.
