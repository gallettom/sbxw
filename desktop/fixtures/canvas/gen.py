#!/usr/bin/env python3
"""Writes the Canvas fixtures (invented sandboxes, invented mock screens).

basic.json: web-app's canvas "Checkout review": round 1 (six desktop shots at
1440, two phone shots at 390), review 1 sent (three pins, a note), round 2
(three shots of the same screens, after), a draft (two pins, a note, an
asset); an older canvas, and one of a removed sandbox. big.json: 15 shots
(12 at 1440x900, 3 at 390x844) for perf.sh and memory.sh. empty.json: a
sandbox with no canvas.
Run: python3 desktop/fixtures/canvas/gen.py
"""
import json, os

HERE = os.path.dirname(os.path.abspath(__file__))
NOW = 1763460000000
DAY = 86400000

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

def shot(pictures, rnd, k, title, note, screen, x, y, w=1440, pw=1440, ph=900, at=NOW):
    f = "s-%06x%06x.png" % (rnd, k)
    pictures[f] = {"w": pw, "h": ph, "seed": rnd * 7 + k}
    h = w * ph / pw
    return {"id": "s-%d-%d" % (rnd, k), "file": f, "round": rnd, "title": title, "note": note, "width": w, "screen": screen,
            "px_w": pw, "px_h": ph, "bytes": 180000 + 1000 * k, "added_ms": at, "x": x, "y": y, "w": w, "h": h}

def checkout(pictures):
    screens = [("Home", "home"), ("Product list", "products"), ("Product page", "product"), ("Cart", "cart"), ("Checkout: address", "address"), ("Checkout: payment", "payment")]
    shots = []
    for k, (t, key) in enumerate(screens):
        shots.append(shot(pictures, 1, k, t, "Logged in, light theme", key, (k % 3) * 1560, (k // 3) * 1140))
    for k, (t, key) in enumerate([("Home, phone", "home-phone"), ("Cart, phone", "cart-phone")]):
        shots.append(shot(pictures, 1, 6 + k, t, "390 wide", key, 3 * 1560 + k * 510, 0, w=390, pw=390, ph=844))
    row2 = 2 * 1140 + 240
    for k, (t, key, x) in enumerate([("Cart (after)", "cart", 0), ("Checkout: address (after)", "address", 1560), ("Checkout: payment (after)", "payment", 3120)]):
        shots.append(shot(pictures, 2, k, t, "After round 1's remarks", key, x, row2, at=NOW + 50 * 60000))
    pins = [
        {"n": 1, "on": "s-1-3", "fx": 0.82, "fy": 0.12, "text": "The total should stay visible while the list scrolls.", "sent": 1},
        {"n": 2, "on": "s-1-4", "fx": 0.3, "fy": 0.55, "text": "Postcode before the city, as on the paper form.", "sent": 1},
        {"n": 3, "on": "s-1-5", "fx": 0.7, "fy": 0.8, "text": "The Pay button is too far down on a laptop.", "sent": 1},
        {"n": 4, "on": "s-2-2", "fx": 0.68, "fy": 0.62, "text": "Better. Make the button full width on narrow screens.", "sent": None},
        {"n": 5, "on": "s-2-0", "fx": 0.2, "fy": 0.3, "text": "Use the example below for the empty cart.", "sent": None},
    ]
    notes = [
        {"id": "n-0000aa01", "n": 1, "x": 4680, "y": 1000, "w": 260, "h": 150, "text": "Overall: fewer borders, more space between groups.", "sent": 1},
        {"id": "n-0000aa02", "n": 2, "x": 4680, "y": 2620, "w": 260, "h": 150, "text": "Round 2 looks closer. Two details left on the payment step.", "sent": None},
    ]
    pictures["a-00000000a001.png"] = {"w": 640, "h": 400, "seed": 9}
    assets = [{"id": "a-1", "n": 1, "file": "a-00000000a001.png", "name": "empty-cart-example.png", "px_w": 640, "px_h": 400, "bytes": 52000,
               "x": 0, "y": row2 + 1140, "w": 480, "h": 300, "sent": None}]
    return {"id": "c-00000000000000c1", "sandbox": "web-app", "title": "Checkout review", "created_ms": NOW, "updated_ms": NOW + 55 * 60000,
            "shots": shots, "assets": assets, "pins": pins, "notes": notes,
            "rounds": [{"n": 1, "at_ms": NOW, "shots": 8}, {"n": 2, "at_ms": NOW + 50 * 60000, "shots": 3}],
            "reviews": [{"n": 1, "at_ms": NOW + 20 * 60000, "session": "web-app · main", "pins": 3, "notes": 1, "assets": 0, "read_ms": NOW + 21 * 60000}]}

def small(pictures, cid, sb, title, at, n=2):
    shots = [shot(pictures, 1, 20 + k, "Screen %d" % (k + 1), "", "s%d" % k, k * 1560, 0, at=at) for k in range(n)]
    return {"id": cid, "sandbox": sb, "title": title, "created_ms": at, "updated_ms": at, "shots": shots, "assets": [], "pins": [], "notes": [],
            "rounds": [{"n": 1, "at_ms": at, "shots": n}], "reviews": []}

def big(pictures):
    shots = []
    for k in range(12):
        shots.append(shot(pictures, 1, 40 + k, "Screen %d" % (k + 1), "", "b%d" % k, (k % 4) * 1560, (k // 4) * 1140))
    for k in range(3):
        shots.append(shot(pictures, 1, 60 + k, "Phone %d" % (k + 1), "", "p%d" % k, 4 * 1560 + k * 510, 0, w=390, pw=390, ph=844))
    return {"id": "c-00000000000000b1", "sandbox": "web-app", "title": "Fifteen screens", "created_ms": NOW, "updated_ms": NOW + DAY,
            "shots": shots, "assets": [], "pins": [], "notes": [], "rounds": [{"n": 1, "at_ms": NOW, "shots": 15}], "reviews": []}

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
