#!/usr/bin/env python3
"""A long run of agent-like terminal output for the memory runs (MEMORY.md):
colours (16, 256 and true colour), bold and underline, wide characters (CJK,
emoji), long lines that wrap, progress lines redrawn in place with \\r and
cursor moves, and a status block redrawn with cursor up and erase. Printed
to stdout, so it goes through a terminal's PTY.

    python3 memory-output.py [LINES] [SEED]     (default 30000 lines)
"""
import random, sys

lines = int(sys.argv[1]) if len(sys.argv) > 1 else 30000
rnd = random.Random(int(sys.argv[2]) if len(sys.argv) > 2 else 1)
E = "\x1b["
words = ("the checkout step reads the cart and the payment form then writes the order "
         "Read Edit Bash Grep tool call result file src/area/File.tsx tests passed failed "
         "warning error done Needs you Irreversible review the diff before merging").split()
wide = ["日本語の文字", "漢字テスト", "한국어", "中文字符", "✅", "🚀", "📦", "⚠️"]
out = sys.stdout
buf = []
def w(s):
    buf.append(s)
    if len(buf) > 200:
        out.write("".join(buf)); buf.clear()

for i in range(lines):
    k = i % 50
    if k == 0:
        # A progress line redrawn in place.
        for p in range(0, 101, 20):
            w(f"\r{E}36m[{'#' * (p // 5):<20}]{E}0m {p:3d}% building")
        w("\r\n")
    elif k == 25:
        # A status block redrawn: three lines up, erased, written again.
        w(f"{E}1;33m● Working{E}0m (esc to interrupt)\r\n  tokens 1234\r\n  time 3s\r\n")
        w(f"{E}3A{E}J{E}1;32m● Done{E}0m\r\n  tokens 2345\r\n  time 4s\r\n")
    else:
        n = rnd.randint(3, 40)
        parts = []
        for _ in range(n):
            t = rnd.choice(words)
            r = rnd.random()
            if r < 0.1:
                parts.append(f"{E}38;5;{rnd.randint(16, 231)}m{t}{E}0m")
            elif r < 0.18:
                parts.append(f"{E}38;2;{rnd.randint(0, 255)};{rnd.randint(0, 255)};{rnd.randint(0, 255)}m{t}{E}0m")
            elif r < 0.24:
                parts.append(f"{E}1;3{rnd.randint(1, 6)}m{t}{E}0m")
            elif r < 0.27:
                parts.append(f"{E}4m{t}{E}24m")
            elif r < 0.31:
                parts.append(rnd.choice(wide))
            else:
                parts.append(t)
        w(f"{E}2m{i:6d}{E}0m " + " ".join(parts) + "\r\n")
w(f"{E}0mmemory-output: {lines} lines\r\n")
out.write("".join(buf))
out.flush()
