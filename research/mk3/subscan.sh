#!/bin/bash
cd "$(dirname "$0")"
P=probe/target/release/mk3-probe
out=scan/20_sub.txt
: > $out
for h in $(seq 0 127); do hh=$(printf %02X $h); for o in 10 20 30 40 50 60 70; do echo "## 20 00 $hh $o" >> $out; $P rq1 "20 00 $hh $o" 150 "00 00 00 10" 2>/dev/null >> $out; done; done
echo done >> $out
