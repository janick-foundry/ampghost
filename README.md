# Katana Control

Katana Control is a desktop editor for the BOSS Katana MK II guitar amplifier, written in Rust.
This repository currently contains Phase 1: the protocol core, parameter maps, MIDI transport,
preset storage, the engine and a diagnostic command line tool.

Supported hardware: Katana 50 MK II (identified by its Universal Identity Reply).
Other models are detected and reported, but every write is refused.

## Safety model

The engine only ever writes to the temporary patch (`60 00 xx xx`) and switches on editor mode
(`7F 00 00 01`). It never stores to amp channels and never writes system memory. Every write is
read back and verified. Channel switching uses MIDI Program Change and discards unsaved edits on
the amp, so the CLI requires `--discard-temp` for it.

## Crates

| Crate | Purpose |
| --- | --- |
| `katana-protocol` | Roland SysEx frames, addresses, identity, device profiles, write plans, verification |
| `katana-params` | Parameter table derived from Katana Mk2 FxFloorBoard (see its NOTICE) |
| `katana-midi` | `MidiTransport` trait, midir backend behind the `native` feature |
| `katana-store` | Preset JSON and `.syx` backup formats, `PresetStorage` trait |
| `katana-engine` | Commands and events, sessions, file storage and engine thread (`native`) |
| `katana-cli` | Diagnostic command line tool |

## Building

Requires Rust 1.99 and the ALSA development files on Linux.

```sh
cargo build --workspace
./check.sh
```

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
