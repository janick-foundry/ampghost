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
| `katana-protocol` | Roland SysEx frames, addresses, identity, device profiles, write plans, verification |
| `katana-params` | Parameter table derived from Katana Mk2 FxFloorBoard (see its NOTICE) |
| `katana-midi` | `MidiTransport` trait, midir backend behind the `native` feature |
| `katana-store` | Preset JSON and `.syx` backup formats, `PresetStorage` trait |
| `katana-engine` | Commands and events, sessions, file storage and engine thread (`native`) |
| `katana-cli` | Diagnostic command line tool |
| `katana-gtk` | GTK 4 / libadwaita desktop app (the only crate with GUI dependencies) |

## Building

Requires Rust 1.99 and the ALSA development files on Linux. The desktop app also needs
GTK 4.22 and libadwaita 1.9 development files.

```sh
cargo build --workspace
./check.sh
```

## Desktop app

```sh
cargo run -p katana-gtk
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
katana-cli detect
katana-cli read-temp [--dump]
katana-cli backup [--out FILE.syx]
katana-cli restore FILE.syx
katana-cli write-temp FILE.syx
katana-cli read-param amp_volume
katana-cli write-param amp_volume 50
katana-cli preset-save "My Lead"
katana-cli preset-apply "My Lead"
katana-cli preset-list
katana-cli switch-channel B1 --discard-temp
```

Presets and default backups live in the user data directory for the app id
`io.github.janfoundry.KatanaControl`.

## License

GPL-3.0-or-later. See `LICENSE` and `NOTICE`.
