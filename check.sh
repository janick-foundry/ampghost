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
    if [[ "${KATANA_DENY_OFFLINE:-0}" == "1" ]]; then
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

step "no automated tests"
if rg -n --type rust -e '#\[(test|cfg\(test\))\]' crates || find crates -type d -name tests | grep -q .; then
    printf 'automated tests are not part of this project\n' >&2
    exit 1
fi

step "single app id definition"
count=$(rg -c --fixed-strings 'io.github.janick_foundry.Tsuba' --type rust crates | awk -F: '{s+=$2} END {print s+0}')
if [[ "$count" != "1" ]]; then
    printf 'app id must be defined exactly once in Rust sources, found %s\n' "$count" >&2
    exit 1
fi

step "core crates stay GUI free (tsuba-gtk excluded)"
if cargo tree --workspace --exclude tsuba-gtk -e normal --prefix none | rg -q '^(gtk|glib|gdk|gio|gsk|pango|cairo|libadwaita|adw|gettext)'; then
    printf 'GUI dependencies found outside tsuba-gtk\n' >&2
    exit 1
fi

printf '\nAll checks passed.\n'
