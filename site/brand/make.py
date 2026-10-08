#!/usr/bin/env python3
"""brnr's mark, drawn on a pixel grid: the phone's LCD, its status row, and
the name in bold pixels. Writes the SVGs next to this file and, with
inkscape, the PNGs:

  icon.svg              the mark (the GitHub org's avatar is icon.svg at 1024 px)
  favicon.svg           16 px, where the name can't be read: the bars and a b
  apple-touch-icon.png  icon.svg at 180 px
  social.svg, .png      1280 x 640, for link previews (og:image, and the
                        repository's social preview, uploaded by hand)

Colours: the dark scheme of site/index.html.
"""

import os
import shutil
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
PAGE, BEZEL, LCD, LCD_INK, LCD_DIM = "#1c1f1a", "#0f110d", "#9fd06a", "#17260c", "#79a64a"

BOLD = {  # 6 wide, 8 tall, 2-pixel stems
    "b": ["11....", "11....", "11111.", "11..11", "11..11", "11..11", "11..11", "11111."],
    "r": ["......", "......", "11111.", "11..11", "11....", "11....", "11....", "11...."],
    "n": ["......", "......", "11111.", "11..11", "11..11", "11..11", "11..11", "11..11"],
}
SMALL = {  # 5 wide; rows 0-1 ascenders, 2-6 x-height, 7-8 descenders
    "a": [".....", ".....", ".111.", "....1", ".1111", "1...1", ".1111"],
    "b": ["1....", "1....", "1111.", "1...1", "1...1", "1...1", "1111."],
    "c": [".....", ".....", ".111.", "1....", "1....", "1....", ".111."],
    "d": ["....1", "....1", ".1111", "1...1", "1...1", "1...1", ".1111"],
    "e": [".....", ".....", ".111.", "1...1", "11111", "1....", ".111."],
    "f": ["..11.", ".1...", "1111.", ".1...", ".1...", ".1...", ".1..."],
    "g": [".....", ".....", ".1111", "1...1", "1...1", "1...1", ".1111", "....1", ".111."],
    "h": ["1....", "1....", "1111.", "1...1", "1...1", "1...1", "1...1"],
    "i": ["..1..", ".....", ".11..", "..1..", "..1..", "..1..", ".111."],
    "n": [".....", ".....", "1111.", "1...1", "1...1", "1...1", "1...1"],
    "o": [".....", ".....", ".111.", "1...1", "1...1", "1...1", ".111."],
    "p": [".....", ".....", "1111.", "1...1", "1...1", "1...1", "1111.", "1....", "1...."],
    "r": [".....", ".....", "1.11.", "11..1", "1....", "1....", "1...."],
    "s": [".....", ".....", ".1111", "1....", ".111.", "....1", "1111."],
    "t": [".1...", ".1...", "1111.", ".1...", ".1...", ".1..1", "..11."],
    "u": [".....", ".....", "1...1", "1...1", "1...1", "1...1", ".1111"],
    "y": [".....", ".....", "1...1", "1...1", "1...1", "1...1", ".1111", "....1", ".111."],
    " ": ["...", "...", "..."],
}


class Grid:
    def __init__(self, w, h, bg):
        self.w, self.h, self.bg, self.px = w, h, bg, {}

    def rect(self, x0, y0, x1, y1, c):
        for x in range(x0, x1 + 1):
            for y in range(y0, y1 + 1):
                self.px[(x, y)] = c

    def glyph(self, font, ch, x0, y0, c, s=1):
        rows = font[ch]
        for dy, row in enumerate(rows):
            for dx, v in enumerate(row):
                if v == "1":
                    self.rect(x0 + dx * s, y0 + dy * s, x0 + dx * s + s - 1, y0 + dy * s + s - 1, c)
        return x0 + len(rows[0]) * s

    def text(self, font, text, x, y, c, s=1, gap=1):
        for ch in text:
            x = self.glyph(font, ch, x, y, c, s) + gap * s
        return x

    def status(self, x0, x1, y0, s, c):
        """Signal bars at x0, the battery ending at x1: 4 units tall from y0."""
        for i in range(4):
            self.rect(x0 + 2 * i * s, y0 + (3 - i) * s, x0 + (2 * i + 1) * s - 1, y0 + 4 * s - 1, c)
        bx = x1 - 7 * s + 1
        self.rect(bx, y0, bx + 6 * s - 1, y0 + 4 * s - 1, c)  # outline...
        self.rect(bx + s, y0 + s, bx + 5 * s - 1, y0 + 3 * s - 1, LCD)  # ...hollow
        self.rect(bx + 2 * s, y0 + s, bx + 3 * s - 1, y0 + 3 * s - 1, c)  # two cells
        self.rect(bx + 4 * s, y0 + s, bx + 5 * s - 1, y0 + 3 * s - 1, c)
        self.rect(x1 - s + 1, y0 + s, x1, y0 + 3 * s - 1, c)  # the nub

    def svg(self, scale):
        out = [
            f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w * scale}" height="{self.h * scale}" shape-rendering="crispEdges">',
            f'<rect width="{self.w}" height="{self.h}" fill="{self.bg}"/>',
        ]
        for c in sorted(set(self.px.values())):  # one path per colour
            d = ""
            for y in range(self.h):
                x = 0
                while x < self.w:
                    if self.px.get((x, y)) == c:
                        x1 = x
                        while self.px.get((x1 + 1, y)) == c:
                            x1 += 1
                        d += f"M{x} {y}h{x1 - x + 1}v1h-{x1 - x + 1}z"
                        x = x1
                    x += 1
            out.append(f'<path fill="{c}" d="{d}"/>')
        out.append("</svg>\n")
        return "\n".join(out)


def width(font, text, gap=1):
    return sum(len(font[ch][0]) + gap for ch in text) - gap


# The icon: 32 x 32.
icon = Grid(32, 32, LCD)
icon.status(3, 28, 3, 1, LCD_INK)
icon.rect(2, 9, 29, 9, LCD_INK)
x = (32 - width(BOLD, "brnr")) // 2 + 1
icon.text(BOLD, "brnr", x, 14, LCD_INK)
icon.rect(2, 26, 29, 26, LCD_DIM)

# Favicon: 16 x 16, where the name can't be read: the bars and a bold b.
fav = Grid(16, 16, LCD)
for i in range(4):
    fav.rect(1 + 2 * i, 4 - i, 1 + 2 * i, 4, LCD_INK)
fav.glyph(BOLD, "b", 8, 6, LCD_INK)
fav.rect(1, 14, 7, 14, LCD_DIM)

# Social preview: 1280 x 640 at 4 px a pixel, the LCD in its bezel.
sp = Grid(320, 160, PAGE)
sp.rect(16, 10, 303, 149, BEZEL)
sp.rect(24, 18, 295, 141, LCD)
sp.status(36, 283, 28, 3, LCD_INK)
sp.rect(32, 44, 287, 45, LCD_INK)
s = 6
sp.text(BOLD, "brnr", (320 - width(BOLD, "brnr") * s) // 2, 56, LCD_INK, s)
line = "a burner phone for your coding agents"
sp.text(SMALL, line, (320 - width(SMALL, line)) // 2, 114, LCD_INK)
sp.rect(32, 131, 287, 131, LCD_DIM)

for name, g, scale in (("icon", icon, 32), ("favicon", fav, 32), ("social", sp, 4)):
    with open(os.path.join(HERE, f"{name}.svg"), "w") as f:
        f.write(g.svg(scale))

if shutil.which("inkscape"):
    for svg, png, w, h in (("icon", "apple-touch-icon", 180, 180), ("social", "social", 1280, 640)):
        out = os.path.join(HERE, f"{png}.png")
        subprocess.run(
            ["inkscape", os.path.join(HERE, f"{svg}.svg"), "-o", out, "-w", str(w), "-h", str(h)],
            check=True,
            capture_output=True,
        )
