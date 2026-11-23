#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

step() { printf '\n==> %s\n' "$*"; }

step "cargo fmt --check"
cargo fmt --all --check

step "cargo clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "cargo build"
cargo build --workspace

step "cargo deny check"
if ! cargo deny check; then
    if [[ "${AMPGHOST_DENY_OFFLINE:-0}" == "1" ]]; then
        printf 'advisory database unavailable, retrying offline without advisories\n'
        cargo deny check --disable-fetch bans licenses sources
    else
        exit 1
    fi
fi

step "no comments in Rust sources"
if rg -n --type rust -e '(^|[^:"\\])//' -e '/\*' -e '#!?\[doc' crates; then
    printf 'comments are not allowed in Rust sources\n' >&2
    exit 1
fi

step "automated tests only in approved places"
test_allowed='^crates/(ampghost-protocol|ampghost-store)/tests/[a-z0-9_]+\.rs$|^crates/ampghost-cli/src/sha256/tests\.rs$|^crates/ampghost-engine/src/native/tests\.rs$'
cfg_allowed='^crates/ampghost-cli/src/sha256\.rs$|^crates/ampghost-engine/src/native\.rs$'
bad=0
while IFS= read -r file; do
    if ! [[ "$file" =~ $test_allowed ]]; then
        printf 'tests are not allowed in %s\n' "$file" >&2
        bad=1
    fi
done < <(rg -l --type rust -e '#\[test\]' crates || true)
while IFS= read -r dir; do
    if ! [[ "$dir" =~ ^crates/(ampghost-protocol|ampghost-store)/tests$ ]]; then
        printf 'tests directory is not allowed: %s\n' "$dir" >&2
        bad=1
    fi
done < <(find crates -type d -name tests -not -path '*/src/*')
while IFS= read -r file; do
    if ! [[ "$file" =~ $cfg_allowed ]] || [[ "$(rg -c --fixed-strings '#[cfg(test)]' "$file")" != "1" ]] \
        || ! rg -q --multiline '^#\[cfg\(test\)\]\nmod tests;\n?\z' "$file"; then
        printf 'cfg(test) is only allowed as a final "mod tests;" in approved modules: %s\n' "$file" >&2
        bad=1
    fi
done < <(rg -l --type rust --fixed-strings 'cfg(test)' crates || true)
if [[ "$bad" != "0" ]]; then
    exit 1
fi

step "cargo test"
cargo test --workspace --all-targets

step "single app id definition"
count=$(rg -c --fixed-strings 'io.github.janick_foundry.AmpGhost' --type rust crates | awk -F: '{s+=$2} END {print s+0}')
if [[ "$count" != "1" ]]; then
    printf 'app id must be defined exactly once in Rust sources, found %s\n' "$count" >&2
    exit 1
fi

step "core crates stay GUI free (ampghost-gtk excluded)"
if cargo tree --workspace --exclude ampghost-gtk -e normal --prefix none | rg -q '^(gtk|glib|gdk|gio|gsk|pango|cairo|libadwaita|adw|gettext)'; then
    printf 'GUI dependencies found outside ampghost-gtk\n' >&2
    exit 1
fi

step "flatpak cargo sources match Cargo.lock"
python3 build-aux/flatpak/check-cargo-sources.py

printf '\nAll checks passed.\n'
