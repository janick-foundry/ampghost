import json
import sys
import tomllib
from pathlib import Path

root = Path(__file__).resolve().parents[2]
lock = tomllib.loads((root / "Cargo.lock").read_text())
sources = json.loads((root / "build-aux/flatpak/cargo-sources.json").read_text())

locked = {
    (f"{package['name']}-{package['version']}", package["checksum"])
    for package in lock.get("package", [])
    if package.get("source", "").startswith("registry+")
}
vendored = {
    (Path(entry["dest"]).name, entry["sha256"])
    for entry in sources
    if entry.get("type") == "archive"
}

if locked != vendored:
    for name, _ in sorted(locked - vendored):
        print(f"missing or changed in cargo-sources.json: {name}", file=sys.stderr)
    for name, _ in sorted(vendored - locked):
        print(f"not in Cargo.lock: {name}", file=sys.stderr)
    print("run build-aux/flatpak/update-cargo-sources.sh", file=sys.stderr)
    sys.exit(1)
