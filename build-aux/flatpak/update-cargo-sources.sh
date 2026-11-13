#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."

generator_rev=41c20aa10819cdb2a4f3ca171758a96d1955c018
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

curl -fsSL -o "$work/flatpak-cargo-generator.py" \
    "https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/${generator_rev}/cargo/flatpak-cargo-generator.py"
python3 -m venv "$work/venv"
"$work/venv/bin/pip" install --quiet --disable-pip-version-check 'aiohttp>=3.9.5,<4' 'PyYAML>=6.0.2,<7' 'tomlkit>=0.13.3,<1'
"$work/venv/bin/python3" "$work/flatpak-cargo-generator.py" Cargo.lock -o build-aux/flatpak/cargo-sources.json
