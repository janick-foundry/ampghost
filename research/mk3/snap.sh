#!/bin/bash
set -u
cd "$(dirname "$0")"
P=probe/target/release/mk3-probe
out="snaps/$1.txt"
mkdir -p snaps
: > "$out"
read_one() { echo "## $1" >> "$out"; "$P" rq1 "$1" 250 "$2" 2>/dev/null >> "$out"; }
read_one "00 00 00 00" "00 00 00 02"
for hh in $(rg -B2 '^F0 41 10 01 05 07 12' scan/20.txt | rg '^## ' | awk '{print $4}'); do read_one "20 00 $hh 00" "00 00 02 00"; done
for hh in $(rg -B2 '^F0 41 10 01 05 07 12' scan/30.txt | rg '^## ' | awk '{print $4}'); do read_one "30 00 $hh 00" "00 00 02 00"; done
echo "$out $(rg -c '^F0' "$out") replies"
