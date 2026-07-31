#!/usr/bin/env python3
"""Print the candidate spinners for eyeball verification in the real terminal.

Codepoints are built from escapes, never literal characters: BMP Private Use
Area glyphs (U+E000-F8FF) get silently stripped by some tooling, which has
already cost this repo one design doc.

The `|` after each glyph is the alignment test. If a glyph is not exactly one
cell, the bars will not form a straight vertical line.
"""

BRAILLE = [0x280B, 0x2819, 0x2839, 0x2838, 0x283C, 0x2834, 0x2826, 0x2827, 0x2807, 0x280F]
ARC = [0xEE06, 0xEE07, 0xEE08, 0xEE09, 0xEE0A, 0xEE0B]
SHIPPED = [0xF04B]          # fa-play, the current in-progress marker
DONE = [0xF070B, 0xF070C]   # md-rhombus filled / outline, the done + todo markers


def block(title, cps, note):
    print(f"\n  {title}")
    print(f"  {note}")
    print("  " + "-" * 46)
    for cp in cps:
        g = chr(cp)
        print(f"    U+{cp:05X}   [{g}]   {g}|{g}|{g}|   tree row: | +- {g} Wire up the watcher")


print("=" * 62)
print("  ALIGNMENT TEST - the | bars must form straight vertical lines")
print("=" * 62)

block("BRAILLE 'dots' spinner (what ora / yarn / npm use)", BRAILLE,
      "measured: AppleBraille substitute, 1.111 cells")
block("NERD FONT arc spinner U+EE06-EE0B", ARC,
      "measured: FiraCodeNF-Reg, 1.000 cells")
block("CURRENTLY SHIPPED in-progress marker", SHIPPED,
      "measured: FiraCodeNF-Reg, 1.000 cells")
block("CURRENTLY SHIPPED done / todo markers", DONE,
      "measured: F070B reports 2.000 cells - worth a look")

print("\n" + "=" * 62)
print("  SIDE BY SIDE, one frame per line, same starting column")
print("=" * 62)
for i in range(6):
    b = chr(BRAILLE[i])
    a = chr(ARC[i])
    print(f"    braille {b} | Wire up the watcher")
    print(f"    arc     {a} | Wire up the watcher")
    print()
