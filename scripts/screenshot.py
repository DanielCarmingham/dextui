#!/usr/bin/env python3
"""Render a dextui screenshot for the README.

The screenshots in docs/img are not photographs of a terminal: they are the
app's own ANSI output, drawn with the real font and the real palette. That
makes them reproducible, and it makes them honest -- nothing here can show a
colour the app does not actually emit.

Reads a `tmux capture-pane -e` dump on stdin and writes a PNG.

    scripts/screenshot.sh          # captures and calls this

Requires: Pillow, and FiraCode Nerd Font installed.
"""

import re
import sys
from PIL import Image, ImageDraw, ImageFont

def find_font(*names):
    """First of `names` that exists in any of the usual font directories.

    Searched rather than hardcoded so this works on someone else's machine --
    Nerd Fonts land in different places depending on whether they came from
    Homebrew, the installer, or a manual unzip.
    """
    import os

    home = os.path.expanduser("~")
    dirs = [
        f"{home}/Library/Fonts/NerdFonts",
        f"{home}/Library/Fonts",
        "/Library/Fonts",
        f"{home}/.local/share/fonts",
        "/usr/share/fonts/truetype",
    ]
    for d in dirs:
        for n in names:
            p = os.path.join(d, n)
            if os.path.exists(p):
                return p
    raise SystemExit(
        f"screenshot.py: none of {names} found in:\n  " + "\n  ".join(dirs)
    )


REGULAR = find_font("FiraCodeNerdFont-Regular.ttf", "FiraCodeNerdFontMono-Regular.ttf")
BOLD = find_font("FiraCodeNerdFont-Bold.ttf", "FiraCodeNerdFontMono-Bold.ttf")

# What macOS substitutes for codepoints the main font lacks.
#
# Pillow draws .notdef -- a tofu box -- for a missing glyph, whereas a terminal
# falls back through the CoreText cascade and draws something. Without this the
# picture would show tofu where the app shows a spinner, which is exactly the
# kind of lie this script exists to avoid.
#
# The braille spinner is the live case: FiraCode Nerd Font contains no braille
# at all, so every frame comes from here. See `icons::BRAILLE_SPIN`.
FALLBACKS = ["/System/Library/Fonts/Apple Braille.ttf"]

# Ghostty's "GitHub Dark Default", the theme this machine actually runs.
BG = (0x0D, 0x11, 0x17)
FG = (0xE6, 0xED, 0xF3)
PALETTE = {
    0: (0x48, 0x4F, 0x58), 1: (0xFF, 0x7B, 0x72), 2: (0x3F, 0xB9, 0x50),
    3: (0xD2, 0x99, 0x22), 4: (0x58, 0xA6, 0xFF), 5: (0xBC, 0x8C, 0xFF),
    6: (0x39, 0xC5, 0xCF), 7: (0xB1, 0xBA, 0xC4), 8: (0x6E, 0x76, 0x81),
    9: (0xFF, 0xA1, 0x98), 10: (0x56, 0xD3, 0x64), 11: (0xE3, 0xB3, 0x41),
    12: (0x79, 0xC0, 0xFF), 13: (0xD2, 0xA8, 0xFF), 14: (0x56, 0xD4, 0xDD),
    15: (0xFF, 0xFF, 0xFF),
}

FONT_SIZE = 17
PAD = 14
SGR = re.compile(r"\x1b\[([0-9;]*)m")


class Cell:
    __slots__ = ("ch", "fg", "bold", "dim", "strike")

    def __init__(self, ch, fg, bold, dim, strike):
        self.ch, self.fg, self.bold, self.dim, self.strike = ch, fg, bold, dim, strike


def parse(text):
    """ANSI -> a grid of styled cells."""
    rows = []
    for line in text.split("\n"):
        fg, bold, dim, strike = FG, False, False, False
        cells = []
        pos = 0
        for m in SGR.finditer(line):
            for ch in line[pos:m.start()]:
                cells.append(Cell(ch, fg, bold, dim, strike))
            pos = m.end()
            codes = [c for c in m.group(1).split(";") if c != ""] or ["0"]
            i = 0
            while i < len(codes):
                c = int(codes[i])
                if c == 0:
                    fg, bold, dim, strike = FG, False, False, False
                elif c == 1:
                    bold = True
                elif c == 2:
                    dim = True
                elif c == 9:
                    strike = True
                elif c == 22:
                    bold = dim = False
                elif c == 29:
                    strike = False
                elif c == 39:
                    fg = FG
                elif 30 <= c <= 37:
                    fg = PALETTE[c - 30]
                elif 90 <= c <= 97:
                    fg = PALETTE[c - 90 + 8]
                elif c == 38 and i + 2 < len(codes) and codes[i + 1] == "5":
                    n = int(codes[i + 2])
                    fg = PALETTE.get(n, FG)
                    i += 2
                i += 1
        for ch in line[pos:]:
            cells.append(Cell(ch, fg, bold, dim, strike))
        rows.append(cells)
    while rows and not "".join(c.ch for c in rows[-1]).strip():
        rows.pop()
    return rows


def coverage(path):
    """Every codepoint a font actually contains."""
    from fontTools.ttLib import TTFont

    f = TTFont(path, fontNumber=0, lazy=True)
    return set().union(*[t.cmap.keys() for t in f["cmap"].tables])


def render(rows, path):
    probe = ImageFont.truetype(REGULAR, FONT_SIZE)
    cw = probe.getlength("M")
    # Ascent+descent, so box-drawing rows meet with no seam between lines.
    asc, desc = probe.getmetrics()
    ch = asc + desc

    cols = max(len(r) for r in rows)
    img = Image.new(
        "RGB",
        (int(cw * cols) + PAD * 2, int(ch * len(rows)) + PAD * 2),
        BG,
    )
    d = ImageDraw.Draw(img)
    reg = ImageFont.truetype(REGULAR, FONT_SIZE)
    bld = ImageFont.truetype(BOLD, FONT_SIZE)

    covered = coverage(REGULAR)

    # A fallback is scaled so its advance matches the cell, which is what a
    # terminal does -- it owns the grid, and a substituted glyph is fitted to it
    # rather than allowed to set its own width. Apple Braille is 1.111 cells at
    # the same point size, so drawn unscaled it would overhang its neighbours.
    spare = []
    for path_ in FALLBACKS:
        probe = ImageFont.truetype(path_, FONT_SIZE)
        natural = probe.getlength("⠋") or cw
        size = max(1, round(FONT_SIZE * cw / natural))
        spare.append((coverage(path_), ImageFont.truetype(path_, size)))

    def font_for(ch, bold):
        if ord(ch) in covered:
            return bld if bold else reg
        for cmap, font in spare:
            if ord(ch) in cmap:
                return font
        return bld if bold else reg  # tofu, and visibly so

    for y, row in enumerate(rows):
        for x, cell in enumerate(row):
            if cell.ch == " ":
                continue
            colour = cell.fg
            if cell.dim:
                colour = tuple(int(v * 0.55 + BG[i] * 0.45) for i, v in enumerate(colour))
            px, py = PAD + x * cw, PAD + y * ch
            font = font_for(cell.ch, cell.bold)
            # A substituted glyph is not the cell's width, and the terminal
            # centres it in the cell rather than letting it push anything along.
            # Doing the same keeps the columns true.
            off = 0
            if font not in (reg, bld):
                w = d.textlength(cell.ch, font=font)
                off = max(0, (cw - w) / 2)
            d.text((px + off, py), cell.ch, font=font, fill=colour)
            if cell.strike:
                mid = py + ch * 0.52
                d.line([(px, mid), (px + cw, mid)], fill=colour, width=1)

    img.save(path)
    print(f"{path}  {img.width}x{img.height}  ({cols}x{len(rows)} cells)")


if __name__ == "__main__":
    render(parse(sys.stdin.read()), sys.argv[1])
