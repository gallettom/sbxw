#!/usr/bin/env python3
"""The desktop app's icon, drawn from the sbxw marks: the browser
extension's tile (a dark rounded square with a pixel grid, the green ">"
prompt and the blue cursor, assets/browser-extension/icons/) in the web UI's
colours (#0d1117 ground, #30363d edge, #3fb950 green, #58a6ff blue, the
favicon's). The macOS grid: an 824 px tile on a 1024 canvas, so it is no
bigger than its Dock neighbours. Small sizes (16, 32: title bars, the
taskbar) fill more of their square and leave out the empty cells.

  python3 desktop/packaging/make-icon.py      # writes icon-*.png and sbxw.icns here

Needs Pillow (a tool for this script only, not a dependency of the app).
"""
import os
import struct
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
BG, EDGE, CELL = (13, 17, 23, 255), (48, 54, 61, 255), (22, 27, 34, 255)
GREEN, BLUE = (63, 185, 80, 255), (88, 166, 255, 255)
# The prompt ">" and the cursor, on a 10 x 10 grid (column, row).
CHEVRON = [(1, 2), (2, 3), (3, 4), (4, 5), (3, 6), (2, 7), (1, 8)]
CURSOR = [(6, 8), (7, 8), (8, 8)]


def draw(px, grid=True):
    s = 4  # supersampling
    W = px * s
    im = Image.new("RGBA", (W, W), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    # The Dock's grid from 64 px up; the window and taskbar sizes fill more.
    tile = W * (824 / 1024 if px >= 64 else 0.94)
    o = (W - tile) / 2
    d.rounded_rectangle([o, o, o + tile, o + tile], radius=tile * 0.225, fill=BG, outline=EDGE, width=max(1, round(W * 10 / 1024)))
    n = 10
    inner = tile * 0.80
    pitch = inner / n
    cell = pitch * 0.80
    g0 = o + (tile - inner) / 2 + (pitch - cell) / 2
    r = cell * 0.18
    lit = {c: GREEN for c in CHEVRON} | {c: BLUE for c in CURSOR}
    for row in range(n):
        for col in range(n):
            colour = lit.get((col, row))
            if colour is None and not grid:
                continue
            x, y = g0 + col * pitch, g0 + row * pitch
            d.rounded_rectangle([x, y, x + cell, y + cell], radius=r, fill=colour or CELL)
    return im.resize((px, px), Image.LANCZOS)


def main():
    sizes = [16, 32, 64, 128, 256, 512, 1024]
    files = {}
    for px in sizes:
        im = draw(px, grid=px >= 64)
        path = os.path.join(HERE, f"icon-{px}.png")
        im.save(path, optimize=True)
        files[px] = open(path, "rb").read()
    # .icns from the PNGs (types per Apple's icns format: PNG payloads).
    kinds = [("icp4", 16), ("icp5", 32), ("icp6", 64), ("ic07", 128), ("ic08", 256), ("ic09", 512), ("ic10", 1024),
             ("ic11", 32), ("ic12", 64), ("ic13", 256), ("ic14", 512)]
    body = b"".join(k.encode() + struct.pack(">I", 8 + len(files[px])) + files[px] for k, px in kinds)
    with open(os.path.join(HERE, "sbxw.icns"), "wb") as f:
        f.write(b"icns" + struct.pack(">I", 8 + len(body)) + body)


if __name__ == "__main__":
    main()
