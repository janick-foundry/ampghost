# Tsuba

Tsuba is a desktop editor for the BOSS Katana MK II guitar amplifier, written in Rust.
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
| `ampghost-protocol` | Roland SysEx frames, addresses, identity, device profiles, write plans, verification |
| `ampghost-params` | Parameter table derived from Katana Mk2 FxFloorBoard (see its NOTICE) |
| `ampghost-midi` | `MidiTransport` trait, midir backend behind the `native` feature |
| `ampghost-store` | Preset JSON and `.syx` backup formats, `PresetStorage` trait |
| `ampghost-engine` | Commands and events, sessions, file storage and engine thread (`native`) |
| `ampghost-cli` | Diagnostic command line tool |
| `ampghost-gtk` | GTK 4 / libadwaita desktop app (the only crate with GUI dependencies) |

## Install

Every GitHub release has a Flatpak bundle attached. Download it and install it for your user:

```sh
flatpak install --user tsuba-<version>-x86_64.flatpak
```

The GNOME runtime is installed from Flathub when needed. The bundle does not update itself;
install the bundle of a newer release to update. Inside the sandbox presets and backups live in
`~/.var/app/io.github.janick_foundry.Tsuba/data/io.github.janick_foundry.Tsuba/`.

To build the Flatpak from source you need `flatpak-builder` (or `org.flatpak.Builder` from
Flathub) and the GNOME 51 SDK with the Rust stable extension:

```sh
flatpak-builder --user --force-clean --install-deps-from=flathub --repo=repo flatpak_app \
    build-aux/flatpak/io.github.janick_foundry.Tsuba.json
flatpak build-bundle repo tsuba.flatpak io.github.janick_foundry.Tsuba \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
```

After changing `Cargo.lock`, regenerate the offline crate sources with
`build-aux/flatpak/update-cargo-sources.sh`. `./check.sh` fails while they are out of date.

## Building

Requires Rust 1.99 and the ALSA development files on Linux. The desktop app also needs
GTK 4.22 and libadwaita 1.9 development files.

```sh
cargo build --workspace
./check.sh
```

## Desktop app

```sh
cargo run -p ampghost-gtk
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
ampghost-cli detect
ampghost-cli read-temp [--dump]
ampghost-cli backup [--out FILE.syx]
ampghost-cli restore FILE.syx
ampghost-cli write-temp FILE.syx
ampghost-cli read-param amp_volume
ampghost-cli write-param amp_volume 50
ampghost-cli preset-save "My Lead"
ampghost-cli preset-apply "My Lead"
ampghost-cli preset-list
ampghost-cli switch-channel B1 --discard-temp
ampghost-cli backup-all --out FILE.alb [--replace]
ampghost-cli backup-all --plan
ampghost-cli alb-info FILE.alb
ampghost-cli tsl-export --out FILE.tsl [--name LIVESET] PRESET...
ampghost-cli tsl-export --out FILE.tsl --from-temp
ampghost-cli tsl-import FILE.tsl [--index N] [--name NAME] [--replace] [--dry-run]
ampghost-cli tsl-import FILE.tsl --index N --to-temp
```

`backup-all` is read only: it sends RQ1 requests for System and the stored patch rows the model
has, one at a time, and saves a BOSS TONE STUDIO `.alb` file. There is no full restore.
`backup-all --plan`, `alb-info`, `tsl-export PRESET...` and `tsl-import` into presets run
without the amp. `tsl-import` checks the device, block lengths and official ranges, ignores the
Status block and fills blocks missing from older files with the factory defaults.

Presets and default backups live in the user data directory for the app id
`io.github.janick_foundry.Tsuba`.

## License

GPL-3.0-or-later. See `LICENSE` and `NOTICE`.
