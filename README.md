<div align="center">

<img src="crates/ampghost-gtk/data/icons/hicolor/scalable/apps/io.github.janick_foundry.AmpGhost.svg" width="128" height="128" alt="AmpGhost icon">

# AmpGhost

Control your BOSS Katana MK II amp from Linux

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](https://github.com/janick-foundry/ampghost/blob/main/LICENSE)
[![Latest release](https://img.shields.io/github/v/release/janick-foundry/ampghost)](https://github.com/janick-foundry/ampghost/releases/latest)
[![Flatpak](https://github.com/janick-foundry/ampghost/actions/workflows/flatpak.yml/badge.svg)](https://github.com/janick-foundry/ampghost/actions/workflows/flatpak.yml)

</div>

AmpGhost is a GTK 4 / libadwaita desktop editor for the BOSS Katana MK II guitar amplifier,
written in Rust. It talks to the amp over USB MIDI and lets you shape the sound of the current
patch from your computer, keep a library of local presets and back up the amp.

## Features

- See the whole signal chain, switch blocks on and off and change the chain order
- Edit amp, booster, mod, FX, delay, reverb, EQ, send/return, noise gate, solo and contour
  parameters live; every change is read back from the amp and verified
- Switch between the Panel, A1, A2, B1 and B2 channels
- Keep local presets: save the current sound, apply, update and delete presets
- Import presets from BOSS TONE STUDIO `.tsl` livesets, as local presets or straight to the amp,
  and export local presets to `.tsl`
- Back up the whole amp (system settings and stored channels) to a BOSS TONE STUDIO `.alb` file
- Automatic backup of the temporary patch before a preset, an imported tone or a reloaded channel
  replaces it

## Supported hardware

| Amp | Status |
| --- | --- |
| Katana 50 MK II | Fully tested and daily driven on my own unit |
| Katana 50 MK III | Currently under analysis, support coming in the future |

I develop and test AmpGhost on my own Katana 50 MK II, connected over USB, and use it with that
amp every day.

AmpGhost runs on Linux. Release bundles are built for x86_64.

## Installation

Download `ampghost-<version>-x86_64.flatpak` from the
[latest release](https://github.com/janick-foundry/ampghost/releases/latest) and install it for
your user:

```sh
flatpak install --user ampghost-<version>-x86_64.flatpak
flatpak run io.github.janick_foundry.AmpGhost
```

> [!NOTE]
> Connect the Katana 50 MK II over USB and turn it on, before or after you start AmpGhost.
> The app finds the amp on its own and reconnects if you unplug it, without losing your unsaved edits.

The bundle needs the GNOME 51 runtime (`org.gnome.Platform//51`) from Flathub. Flatpak installs
it together with the bundle when it can reach Flathub; if the Flathub remote is not set up yet,
add it first:

```sh
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
```

After installation AmpGhost also appears in your application launcher.

## Update

Flatpak bundles do not update themselves. Download the bundle of the newer release and install it
over the current one:

```sh
flatpak install --user --reinstall ampghost-<version>-x86_64.flatpak
```

Your presets and backups are kept.

## Uninstall

```sh
flatpak uninstall --user io.github.janick_foundry.AmpGhost
```

Presets and backups stay in `~/.var/app/io.github.janick_foundry.AmpGhost/`. Add `--delete-data`
to remove them as well, and run `flatpak uninstall --user --unused` to remove the runtime if no
other app needs it.

## Tone files (.tsl)

BOSS Tone Studio stores livesets in `.tsl` files. A liveset holds one or more patches, and
AmpGhost can import and export livesets for the Katana 50 MK II.

### Where to find them

- [BOSS Tone Exchange](https://bosstoneexchange.com/gear/katana_mk2/): livesets shared by Katana
  MK II players. Downloading needs a free Roland account.
- [GuitarPatches](https://www.guitarpatches.com/patches.php?unit=KATANAMKII): a community library
  of Katana MK II patches. Some downloads are zipped, so unpack the `.tsl` file first.

### Import

Choose **Import Presets (.tsl)…** in the main menu and open a file. If the liveset holds several
patches, pick one from the list. AmpGhost shows what it found, for example blocks that are missing
in older files and get filled with factory defaults, and then offers:

- **Save as Local Preset**: keeps the patch on your computer under a name you choose. This works
  without the amp connected.
- **Send to Temp**: backs up the amp's temporary patch and then loads the imported patch into it.
  Your amp channels are not changed.

Patches are imported one at a time. AmpGhost only accepts livesets made for the Katana MK II; a
file for the Katana 50 MK III, or a patch with values outside the official ranges, is refused with
an error.

### Export

Choose **Export Presets (.tsl)…** in the main menu, tick up to 15 local presets, name the liveset
and choose where to save it. You can open the file in BOSS Tone Studio or import it into AmpGhost
again. Export works without the amp connected.

## Safety

- AmpGhost only writes to the amp's temporary patch. It never stores to the amp's channels and
  never writes system settings.
- Before a preset, an imported tone or a reloaded channel replaces the temporary patch, the current
  temporary patch is backed up automatically. Single parameter edits are written directly.
- Every write is read back from the amp and verified.
- Switching channels discards unsaved edits on the amp, so AmpGhost asks for confirmation when
  the current sound has unsaved changes.

<details>
<summary>Build from source</summary>

Requires Rust 1.99, the ALSA development files and the GTK 4.22 and libadwaita 1.9 development
files.

```sh
cargo build --release
./target/release/ampghost
```

`./check.sh` runs formatting, clippy, the build, `cargo deny` and the project checks.

To build the Flatpak bundle you need `flatpak-builder` and the GNOME 51 SDK with the Rust stable
extension from Flathub:

```sh
flatpak-builder --user --force-clean --install-deps-from=flathub --repo=repo flatpak_app \
    build-aux/flatpak/io.github.janick_foundry.AmpGhost.json
flatpak build-bundle repo ampghost.flatpak io.github.janick_foundry.AmpGhost \
    --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
```

After changing `Cargo.lock`, regenerate the offline crate sources with
`build-aux/flatpak/update-cargo-sources.sh`.

The repository also contains `ampghost-cli`, a diagnostic command line tool
(`cargo run -p ampghost-cli -- --help`).

</details>

## License

AmpGhost is licensed under the GNU General Public License, version 3 or later
(GPL-3.0-or-later). See [LICENSE](LICENSE) and [NOTICE](NOTICE).

The parameter maps are derived from Katana Mk2 FxFloorBoard by Colin Willcocks (gumtown), which is
distributed under the GNU General Public License. Full attribution is in
[crates/ampghost-params/NOTICE](crates/ampghost-params/NOTICE).

BOSS and KATANA are trademarks of Roland Corporation. AmpGhost is not affiliated with or endorsed
by Roland Corporation.
