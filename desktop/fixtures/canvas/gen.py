#!/usr/bin/env python3
"""Writes the Canvas fixtures (invented sandboxes, invented mock screens).

basic.json: web-app's canvas "Checkout review": round 1 (six desktop shots at
1440, two phone shots at 390), review 1 sent (three pins, a note), round 2
(three shots of the same screens, after), a draft (two pins, a note, an
asset); an older canvas, and one of a removed sandbox. big.json: 15 shots
(12 at 1440x900, 3 at 390x844) for perf.sh and memory.sh. empty.json: a
sandbox with no canvas. grid12.json: one round of 12 desktop shots at
1284 (the user's case); grid2.json: that round and a second of six shots,
five of the same screens (same cells) and a new one.
Positions follow the core's grid (crates/core/src/canvas.rs, layout_round):
cells as wide as the widest shot, phones three to a cell, columns from
grid_cols, rows GRID_ROW_GAP apart, rounds ROW_GAP apart, an empty row of a
later round taking no room.
Run: python3 desktop/fixtures/canvas/gen.py
"""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))
NOW = 1763460000000
DAY = 86400000

COL_GAP, GRID_ROW_GAP, ROW_GAP, NARROW_GAP = 120, 200, 320, 48
WHOLE = 4294967295

def grid_cols(cells, cw, ch):
    """The core's grid_cols: the column count whose block is nearest 1.6 wide."""
    import math
    if cells <= 1:
        return 1
    best = (1e9, 1)
    for c in range(1, min(cells, 8) + 1):
        rows = -(-cells // c)
        w = c * cw + (c - 1) * COL_GAP
        h = rows * ch + (rows - 1) * GRID_ROW_GAP
        cost = abs(math.log(w / h / 1.6)) + 0.05 * (rows * c - cells)
        if cost < best[0] - 1e-4:
            best = (cost, c)
    return best[1]

def place(shots, cells, top, cw, sw=0):
    """Puts each shot at its cell [row, col, slot]; returns the round's bottom."""
    rows = sorted({c[0] for c in cells})
    y, row_y = top, {}
    for r in range(max(rows) + 1):
        row_y[r] = y
        if r in rows:
            y += max(s["h"] for s, c in zip(shots, cells) if c[0] == r) + GRID_ROW_GAP
    for s, c in zip(shots, cells):
        s["x"] = c[1] * (cw + COL_GAP) + (0 if c[2] == WHOLE else c[2] * (sw + NARROW_GAP))
        s["y"] = row_y[c[0]]
        s["cell"] = c
    return max(s["y"] + s["h"] for s in shots)

def sandboxes():
    return [
        {"name": "web-app", "agent": "claude", "status": "running", "workspace": "/work/web", "created_at": "2026-11-10T09:12:00Z", "approvals": 0, "kits_waiting": 0},
        {"name": "docs", "agent": "claude", "status": "stopped", "workspace": "/work/docs", "created_at": "2026-11-11T09:12:00Z", "approvals": 0, "kits_waiting": 0},
    ]

def sessions():
    return [
        {"sandbox": "web-app", "mode": "claude", "state": "idle", "agent": "claude", "started_ms": NOW + 5 * 60000, "ts": NOW + 60 * 60000, "answerable": True, "origin": "sbxw", "session_id": "s-main"},
        {"sandbox": "web-app", "mode": "claude", "state": "working", "agent": "claude", "started_ms": NOW + 7 * 60000, "ts": NOW + 60 * 60000, "answerable": True, "thread": "checkout-ui", "origin": "sbxw", "session_id": "s-thread"},
    ]

def shot(pictures, rnd, k, title, note, screen, x, y, w=1440, pw=1440, ph=900, at=NOW, dark=False):
    f = "s-%06x%06x.png" % (rnd, k)
    pictures[f] = {"w": pw, "h": ph, "seed": rnd * 7 + k}
    if dark:
        pictures[f]["dark"] = True
    h = w * ph / pw
    return {"id": "s-%d-%d" % (rnd, k), "file": f, "round": rnd, "title": title, "note": note, "width": w, "screen": screen,
            "px_w": pw, "px_h": ph, "bytes": 180000 + 1000 * k, "added_ms": at, "x": x, "y": y, "w": w, "h": h}

def checkout(pictures):
    screens = [("Home", "home"), ("Product list", "products"), ("Product page", "product"), ("Cart", "cart"), ("Checkout: address", "address"), ("Checkout: payment", "payment")]
    r1 = [shot(pictures, 1, k, t, "Logged in, light theme", key, 0, 0) for k, (t, key) in enumerate(screens)]
    r1 += [shot(pictures, 1, 6 + k, t, "390 wide", key, 0, 0, w=390, pw=390, ph=844) for k, (t, key) in enumerate([("Home, phone", "home-phone"), ("Cart, phone", "cart-phone")])]
    # Six whole cells and one cell of two phones: 3 columns.
    assert grid_cols(7, 1440, 900) == 3
    bottom = place(r1, [[k // 3, k % 3, WHOLE] for k in range(6)] + [[2, 0, 0], [2, 0, 1]], 0, 1440, 390)
    row2 = bottom + ROW_GAP
    r2 = [shot(pictures, 2, k, t, "After round 1's remarks", key, 0, 0, at=NOW + 50 * 60000) for k, (t, key) in enumerate([("Cart (after)", "cart"), ("Checkout: address (after)", "address"), ("Checkout: payment (after)", "payment")])]
    # The same screens in the same cells (row 1); row 0 is empty in round 2 and takes no room.
    place(r2, [[1, 0, WHOLE], [1, 1, WHOLE], [1, 2, WHOLE]], row2, 1440)
    shots = r1 + r2
    pins = [
        {"n": 1, "on": "s-1-3", "fx": 0.82, "fy": 0.12, "text": "The total should stay visible while the list scrolls.", "sent": 1},
        {"n": 2, "on": "s-1-4", "fx": 0.3, "fy": 0.55, "text": "Postcode before the city, as on the paper form.", "sent": 1},
        {"n": 3, "on": "s-1-5", "fx": 0.7, "fy": 0.8, "text": "The Pay button is too far down on a laptop.", "sent": 1},
        {"n": 4, "on": "s-2-2", "fx": 0.68, "fy": 0.62, "text": "Better. Make the button full width on narrow screens.", "sent": None},
        {"n": 5, "on": "s-2-0", "fx": 0.2, "fy": 0.3, "text": "Use the example below for the empty cart.", "sent": None},
    ]
    notes = [
        {"id": "n-0000aa01", "n": 1, "x": 4680, "y": 1100, "w": 260, "h": 150, "text": "Overall: fewer borders, more space between groups.", "sent": 1},
        {"id": "n-0000aa02", "n": 2, "x": 4680, "y": row2, "w": 260, "h": 150, "text": "Round 2 looks closer. Two details left on the payment step.", "sent": None},
    ]
    pictures["a-00000000a001.png"] = {"w": 640, "h": 400, "seed": 9}
    assets = [{"id": "a-1", "n": 1, "file": "a-00000000a001.png", "name": "empty-cart-example.png", "px_w": 640, "px_h": 400, "bytes": 52000,
               "x": 4680, "y": row2 + 300, "w": 480, "h": 300, "sent": None}]
    return {"id": "c-00000000000000c1", "sandbox": "web-app", "title": "Checkout review", "created_ms": NOW, "updated_ms": NOW + 55 * 60000,
            "shots": shots, "assets": assets, "pins": pins, "notes": notes,
            "rounds": [{"n": 1, "at_ms": NOW, "shots": 8, "cols": 3, "cell_w": 1440, "slot_w": 390}, {"n": 2, "at_ms": NOW + 50 * 60000, "shots": 3, "cols": 3, "cell_w": 1440, "slot_w": 390}],
            "reviews": [{"n": 1, "at_ms": NOW + 20 * 60000, "session": "web-app · main", "pins": 3, "notes": 1, "assets": 0, "read_ms": NOW + 21 * 60000}]}

def small(pictures, cid, sb, title, at, n=2):
    shots = [shot(pictures, 1, 20 + k, "Screen %d" % (k + 1), "", "s%d" % k, 0, 0, at=at) for k in range(n)]
    cols = grid_cols(n, 1440, 900)
    place(shots, [[k // cols, k % cols, WHOLE] for k in range(n)], 0, 1440)
    return {"id": cid, "sandbox": sb, "title": title, "created_ms": at, "updated_ms": at, "shots": shots, "assets": [], "pins": [], "notes": [],
            "rounds": [{"n": 1, "at_ms": at, "shots": n, "cols": cols, "cell_w": 1440}], "reviews": []}

def big(pictures):
    shots = [shot(pictures, 1, 40 + k, "Screen %d" % (k + 1), "", "b%d" % k, 0, 0) for k in range(12)]
    shots += [shot(pictures, 1, 60 + k, "Phone %d" % (k + 1), "", "p%d" % k, 0, 0, w=390, pw=390, ph=844) for k in range(3)]
    # Twelve whole cells and one of three phones: 4 columns.
    assert grid_cols(13, 1440, 900) == 4
    place(shots, [[k // 4, k % 4, WHOLE] for k in range(12)] + [[3, 0, k] for k in range(3)], 0, 1440, 390)
    return {"id": "c-00000000000000b1", "sandbox": "web-app", "title": "Fifteen screens", "created_ms": NOW, "updated_ms": NOW + DAY,
            "shots": shots, "assets": [], "pins": [], "notes": [], "rounds": [{"n": 1, "at_ms": NOW, "shots": 15, "cols": 4, "cell_w": 1440, "slot_w": 390}], "reviews": []}

APP_SCREENS = [("Dashboard", "dashboard"), ("Inbox", "inbox"), ("Message", "message"), ("Compose", "compose"), ("Contacts", "contacts"), ("Contact card", "contact"),
               ("Calendar", "calendar"), ("Event", "event"), ("Files", "files"), ("Search results", "search"), ("Settings", "settings"), ("Sign in", "signin")]

def grid(pictures, second):
    """A round of 12 desktop shots at 1284 (and, `second`, a round of six after)."""
    r1 = [shot(pictures, 1, 80 + k, t, "1284 wide, dark theme", key, 0, 0, w=1284, pw=2568, ph=1632, dark=True) for k, (t, key) in enumerate(APP_SCREENS)]
    assert grid_cols(12, 1284, 816) == 4
    bottom = place(r1, [[k // 4, k % 4, WHOLE] for k in range(12)], 0, 1284)
    rounds = [{"n": 1, "at_ms": NOW - 2 * 60000, "shots": 12, "cols": 4, "cell_w": 1284}]
    shots = r1
    if second:
        again = [("Inbox (after)", "inbox", [0, 1]), ("Compose (after)", "compose", [0, 3]), ("Contacts (after)", "contacts", [1, 0]), ("Files (after)", "files", [2, 0]), ("Settings (after)", "settings", [2, 2]), ("Archive (new)", "archive", None)]
        r2 = [shot(pictures, 2, 100 + k, t, "After round 1's remarks", key, 0, 0, w=1284, pw=2568, ph=1632, at=NOW + 30 * 60000, dark=True) for k, (t, key, _) in enumerate(again)]
        # Round 1 filled its 12 cells: the new screen goes after them (row 3).
        cells = [[c[0], c[1], WHOLE] if c else [3, 0, WHOLE] for _, _, c in again]
        place(r2, cells, bottom + ROW_GAP, 1284)
        rounds.append({"n": 2, "at_ms": NOW + 30 * 60000, "shots": 6, "cols": 4, "cell_w": 1284})
        shots = r1 + r2
    title = "Mail app, two rounds" if second else "Mail app"
    return {"id": "c-00000000000000g%d" % (2 if second else 1), "sandbox": "web-app", "title": title, "created_ms": NOW, "updated_ms": NOW + DAY,
            "shots": shots, "assets": [], "pins": [], "notes": [], "rounds": rounds, "reviews": []}

def write(name, canvases, pictures, selected="web-app"):
    fx = {"about": "Canvas: invented sandboxes and mock screens (gen.py).",
          "app": {"selected": selected, "panes": 1, "split": "columns", "open": ["web-app"], "focused": 1},
          "daemon": {"sandboxes": sandboxes(), "sessions": sessions(), "canvas": {"canvases": {c["id"]: c for c in canvases}, "pictures": pictures}}}
    with open(os.path.join(HERE, name + ".json"), "w") as f:
        json.dump(fx, f, indent=1, ensure_ascii=False)
        f.write("\n")

p = {}
write("basic", [checkout(p), small(p, "c-00000000000000c2", "web-app", "Settings pages", NOW - 40 * DAY), small(p, "c-00000000000000c3", "old-shop", "Old shop review", NOW - 100 * DAY)], p)
p = {}
write("big", [big(p)], p)
write("empty", [], {})
p = {}
write("grid12", [grid(p, False)], p)
p = {}
write("grid2", [grid(p, True)], p)
