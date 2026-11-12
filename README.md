# Katana Control

Katana Control is a desktop editor for the BOSS Katana MK II guitar amplifier, written in Rust.
This repository contains the protocol core, parameter maps, MIDI transport, preset storage, the
engine, a diagnostic command line tool (Phase 1) and a GTK 4 / libadwaita desktop app (Phase 2).

Supported hardware: Katana 50 MK II (identified by its Universal Identity Reply).
Other models are detected and reported, but every write is refused.

## Safety model

The engine only ever writes to the temporary patch (`60 00 xx xx`) and switches on editor mode
(`7F 00 00 01`). It never stores to amp channels and never writes system memory. Every write is
read back and verified. Channel switching uses MIDI Program Change and discards unsaved edits on
the amp, so the CLI requires `--discard-temp` for it and the desktop app asks for confirmation.
Saving the temporary patch to an amp channel is not implemented.

## Crates

| Crate | Purpose |
| --- | --- |
| `tsuba-protocol` | Roland SysEx frames, addresses, identity, device profiles, write plans, verification |
| `tsuba-params` | Parameter table derived from Katana Mk2 FxFloorBoard (see its NOTICE) |
| `tsuba-midi` | `MidiTransport` trait, midir backend behind the `native` feature |
| `tsuba-store` | Preset JSON and `.syx` backup formats, `PresetStorage` trait |
| `tsuba-engine` | Commands and events, sessions, file storage and engine thread (`native`) |
| `tsuba-cli` | Diagnostic command line tool |
| `tsuba-gtk` | GTK 4 / libadwaita desktop app (the only crate with GUI dependencies) |

## Building

Requires Rust 1.99 and the ALSA development files on Linux. The desktop app also needs
GTK 4.22 and libadwaita 1.9 development files.

```sh
cargo build --workspace
./check.sh
```

## Desktop app

```sh
cargo run -p tsuba-gtk
```

The header bar holds the channel buttons, the current patch name, the connection state and the
progress of the running operation. The sidebar lists local presets (click to apply to the
temporary patch), the centre shows the signal chain with on/off switches and variant colours, and
the lower panel edits the parameters of the selected block. Every edit is written to the
temporary patch and verified; slider edits are debounced. The main menu offers temp backup and
restore. Before a preset is applied or a backup is restored, the current temporary patch is
backed up automatically. Unknown models are shown with their identity and stay read only.

## CLI

```sh
tsuba-cli detect
tsuba-cli read-temp [--dump]
tsuba-cli backup [--out FILE.syx]
tsuba-cli restore FILE.syx
tsuba-cli write-temp FILE.syx
tsuba-cli read-param amp_volume
tsuba-cli write-param amp_volume 50
tsuba-cli preset-save "My Lead"
tsuba-cli preset-apply "My Lead"
tsuba-cli preset-list
tsuba-cli switch-channel B1 --discard-temp
tsuba-cli backup-all --out FILE.alb [--replace]
tsuba-cli backup-all --plan
tsuba-cli alb-info FILE.alb
tsuba-cli tsl-export --out FILE.tsl [--name LIVESET] PRESET...
tsuba-cli tsl-export --out FILE.tsl --from-temp
tsuba-cli tsl-import FILE.tsl [--index N] [--name NAME] [--replace] [--dry-run]
tsuba-cli tsl-import FILE.tsl --index N --to-temp
```

`backup-all` is read only: it sends RQ1 requests for System and the stored patch rows the model
has, one at a time, and saves a BOSS TONE STUDIO `.alb` file. There is no full restore.
`backup-all --plan`, `alb-info`, `tsl-export PRESET...` and `tsl-import` into presets run
without the amp. `tsl-import` checks the device, block lengths and official ranges, ignores the
Status block and fills blocks missing from older files with the factory defaults.

Presets and default backups live in the user data directory for the app id
`io.github.janfoundry.KatanaControl`.

## License

GPL-3.0-or-later. See `LICENSE` and `NOTICE`.
