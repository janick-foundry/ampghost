# Katana MK III (Gen 3) read-only probe, 2026-10-31

All traffic: identity request + RQ1 (op 11) only. No DT1, PC, CC sent. Tool: probe/ (midir, ALSA seq client, refuses anything else).

## Sources
- vguitarforums "katana Gen 3 Sysex Reverse Engeeniering" https://www.vguitarforums.com/smf/index.php?topic=38729.0 (forum posts, no licence; attached BTS js = BOSS proprietary, not reused)
- kevlu123/libkatana https://github.com/kevlu123/libkatana GPL-3.0, commit 6515be2 (2026-03-26): Gen3 header, addresses
- katana-dev/lib-katana https://github.com/katana-dev/lib-katana (MK I/II)
- snhirsch/katana-midi-bridge doc/katana_sysex.txt (MK I)
- FxFloorBoard Gen3 folder on SourceForge: empty ("Nothing here yet")

## Results
- Identity: F0 7E 10 06 02 41 07 05 00 00 0A 00 00 00 F7 (device 10, family 07 05, rev 0A 00 00 00)
- MK II header (..00 33) RQ1 60 00 00 00: no answer. Gen3 header at 60 00 00 00: no answer.
- Gen3 RQ1 answered WITHOUT editor mode.
- 00 00 00 00 (2): 00 01 = current channel (libkatana RO_CHANNEL)
- 00 01 00 00, 7F 01 01 00: no answer
- Scan RQ1 size 00 00 01 00 at XX 00 hh 00, hh=00..7F, XX=00,10,20,30: see scan/*.txt, dump_all_replies.syx
  - 20 (temp patch): 70 replies, 2113 bytes; blocks at even hh; 1C..27 are 225-byte blocks (128 + 97)
  - 10: 21 replies, 131 bytes (system?); 30: 3 replies, 7 bytes (30 00 02 00..04 = FX knob levels per libkatana)
- Decoded (current patch): name 20 00 00 00 "Chunky Brown"; 20 00 02 01 cab res 01 Modern; 20 00 04 00..04 colours 00 01 02 02 02;
  20 00 06 00..09 gain 47 vol 33 bass 4A mid 2D treble 2D presence 36 bloom 00 type 05 Brown resonance 32 variation 01; solo 20 00 3A 00 = 00; contour 20 00 40 00 = 00

## Open
- Blocks starting at hh 40 etc. not scanned (only hh 00); odd pages beyond 225-byte blocks empty
- Meaning of most blocks; 10 xx area; 7F 01 01 0x reads
- Unsolicited DT1 behaviour on knob turn / channel switch

## Live mapping (Jan turns panel, read-only snapshots via snap.sh + diff.py), 2026-10-31
- No unsolicited SysEx from the amp on panel changes without editor mode (aseqdump shows only our replies).
- 20 00 06 07 = Amp Type (panel knob): 0 Acoustic, 1 Clean, 2 Pushed, 3 Crunch, 4 Lead, 5 Brown. Verified all six live; only that byte changes.
- 20 00 06 09 = Variation: 1 on, 0 off (verified live).
- 20 00 06 00 = Gain 0..100 (verified min 0 and max 100 live).
- 20 00 06 01 = Volume, 06 02 = Bass, 06 03 = Middle (verified live, 0..100).
- 20 00 06 04 = Treble (verified live).
- Sub-page scan (20 00 hh 10..70, 16-byte reads): replies only inside pages 1C-27, so page-start snapshots already cover area 20. Booster/Mod knob to OFF changed only 20 00 1F 45: 0 -> 5.
- Booster knob OFF -> ~12h (green LED): 20 00 08 00 0->1 (booster on/off, likely), 20 00 0A 01 0->59 (booster block 0A, likely drive; 0A 00 = type 02), 30 00 02 00 0->47 (panel knob position, matches libkatana KNOB_FX_BOOSTER).
- Mod knob ~half: 20 00 08 01 0->1 (mod on/off), 20 00 1F 45 5->50 (mod param driven by knob; was 5 at OFF), 30 00 02 01 0->51 (knob position). Pattern: 20 00 08 0x = effect on/off flags.
- FX knob ~half (orange/yellow LED): 20 00 08 02 0->1 (fx on/off), 20 00 27 48 48->46 (fx param driven by knob), 30 00 02 02 0->45 (knob position).
- Delay knob 13h -> OFF: 20 00 08 03 1->0 (delay on/off), 20 00 2C 07 27->0 (likely delay level), 20 00 2C 05 33->35 (unclear, maybe feedback). 30 00 02 03 stayed 0 (was 0 even at 13h) - knob position area not reliable for delay.
- Reverb knob -> OFF: 20 00 08 05 1->0 (reverb on/off; note 08 04 is NOT reverb, unknown - maybe a hidden block like delay 2/send-return), 20 00 38 0A 77->0 (reverb level), 30 00 02 04 57->0 (knob position).
- Channel: 00 00 00 01 = current channel, CH1=1, CH2=2 (verified CH1->CH2 by panel button). Switching loads the whole temp area 20 (139 bytes changed); name at 20 00 00 00 (16 chars): CH1 'Chunky Brown', CH2 'Picking Pushed'.
- PANEL: 00 00 00 01 = 0 (verified). Panel patch name 'KATANA'. 120 bytes changed vs CH2. Panel amp 20 00 06 00..09 = 100,0,0,100,0,50,0,0,50,0 (to be confirmed against physical knobs).
- PANEL values confirmed by Jan against physical knobs (gain max, vol 0, bass 0, middle max, treble 0, Acoustic). Baseline for further tests: s20_panel.
- BOOSTER button press (s20->s21): 20 00 04 00 2->0 (likely booster colour slot, now green=0), booster params reloaded: 0A 01 19->32, 0A 06 66->59. On flag 08 00 unchanged.
- BOOSTER colour 20 00 04 00 verified: 0 green, 1 red, 2 orange; button cycles orange->green->red. Each colour has its own block: green 20 00 0A xx, red 20 00 0C xx (orange likely 0E, unverified). In Panel mode, selecting a colour writes the current knob value into that block (01: 19->32, 06 also changes).
- Back to orange: only 04 00 1->2 changed. Colour blocks: 0A green [9,32,60,50,0,50,59,0], 0C red [10,32,73,50,0,50,64,0], 0E orange [13,28,68,50,0,50,51,0]; byte 00 = booster type per colour. Earlier 'knob value written on colour select' theory NOT confirmed (orange unchanged, 0E 01=28).
- BOOSTER knob max (orange, Panel): 0E 01 28->60, 0E 06 51->50, panel pos 30 00 02 00 47->101. Knob writes into currently selected colour block; one knob drives several params (macro curve).
- BOOSTER knob min (orange): 08 00 1->0 (knob at min = off), 0E 01 60->0, 0E 06 50->65 (06 is not monotonic: 65 min, 51 mid, 50 max). Accidental MOD bump: 30 00 02 01 51->50 with 1D 37 50->49 and 1D 3B 40->39 (mod knob drives params in block 1D for current mod type).
- MOD colour 20 00 04 01: 0 green, 1 red (verified). Green->red changed only 1F 45 43->49 besides colour. Mod knob param seems in 1D (green, 1D 37/3B) and 1F (red, 1F 45) — per-colour mod parameter sets in big pages 1C-27.
- MOD orange: 04 01 = 2 verified; red->orange changed 21 48 45->49. MOD per-colour sets: green ~1D, red ~1F, orange ~21.
- FX colour 20 00 04 02: 0 green, 1 red verified; green->red changed 24 02 50->32. Earlier FX knob (orange LED) changed 27 48.
- FX orange: 04 02 = 2 verified; red->orange changed 27 5A 50->44. FX orange set ~27 confirmed (knob earlier 27 48).
- DELAY (Panel): knob 0->13:00 then button green->red. 08 03 0->1 (on), colour 04 03 0->1. Green set 28 (28 05 35->27, 28 07 0->77), red set 2A (2A 05 22->27, 2A 06 10->8, 2A 07 50->77). Panel pos 30 00 02 03 0->64 (in Panel mode area 30 does track delay; on CH1 earlier it stayed 0). Orange delay set likely 2C (seen on CH1 earlier). Knob values appear copied into newly selected colour set.
- DELAY orange: 04 03 = 2 verified; set 2C (2C 05 22->27, 2C 07 50->77). Delay sets: green 28, red 2A, orange 2C. Colour switch copies knob-derived values into the new set (confirmed for delay).
- REVERB (Panel): knob to half, green->red. On flag 08 05 0->1; colour 20 00 04 04 0->1 (note: colour index 04 04, on flag 08 05; 08 04 is something else, maybe delay 2). Green set 34 (34 0A 0->66), red set 36 (36 0A 35->66, 36 09 8->5). Panel pos 30 00 02 04 0->50. Orange likely 38 (CH1 reverb off changed 38 0A).
- REVERB orange: 04 04 = 2 verified; set 38 (38 07 14->2, 38 09 8->10, 38 0A 35->73 — level differs from green/red 66, so knob curve depends on reverb type). SUMMARY colours: 20 00 04 00..04 = booster/mod/fx/delay/reverb colour (0 green,1 red,2 orange). Per-colour sets: booster 0A/0C/0E, mod ~1D/1F/21, fx ~?/24/27, delay 28/2A/2C, reverb 34/36/38.
- TAP (3 taps, delay orange): 2C 02 1->6, 2C 03 9->14. Hypothesis delay time = 7-bit pair (hi*128+lo) ms: 137 -> 782 ms. Needs second tap test to confirm encoding.
- TAP fast: 2C 01..04 = 00 01 08 07. Correct encoding: delay time = 4 nibbles at 2C 01..04 (Roland nibble format), ms: before 0x0190=400, slow taps 0x06E0=1760, fast taps 0x0187=391. 7-bit hypothesis rejected. 2C 00 = 04 (delay type). Green set time likely 28 01..04 (matches libkatana).
- STEP 4 (approved by Jan 2026-10-31): backup w00_before sha 810f6aa3...6206. DT1 20 00 06 00 = 32 sent via amidi hw:2,0,0 WITHOUT editor mode; readback shows gain 50 -> MK III accepts DT1 without editor mode.
- Restore: gain back to 100. After-snapshot w01_after differs from backup only in 20 00 06 01 Volume 0->23 (not written by me; likely Jan turned the volume knob). Gain test: write+audible change+restore OK.
