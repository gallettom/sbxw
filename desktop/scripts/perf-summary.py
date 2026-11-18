#!/usr/bin/env python3
"""Sums SBXW_DESKTOP_PERF's log lines per step of scripts/perf.sh."""
import re, sys
from collections import defaultdict
step, order, acc = "start", ["start"], defaultdict(lambda: defaultdict(lambda: [0, 0.0, 0.0]))
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"perf: (.*) ([\d.]+) ms\s*$", line)
    if "perf: ---- " in line:
        step = line.split("perf: ---- ", 1)[1].strip(); order.append(step); continue
    if not m: continue
    what = m.group(1)
    what = re.sub(r"\(screen_data [\d.]+ ms, \d+ bytes; (\d+) of (\d+) screens rendered\)", r"(\1 of \2 screens)", what)
    what = re.sub(r"/api/sandboxes/[^/ ]+/commits/[0-9a-f]+", "/api/sandboxes/:name/commits/:hash", what)
    what = re.sub(r"/api/sandboxes/[^/ ?]+/", "/api/sandboxes/:name/", what)
    what = re.sub(r"\?\S*", "", what)
    a = acc[step][what]; ms = float(m.group(2)); a[0] += 1; a[1] += ms; a[2] = max(a[2], ms)
for s in order:
    if s not in acc: continue
    print(f"\n== {s}")
    for what, (n, tot, mx) in sorted(acc[s].items(), key=lambda kv: -kv[1][1])[:12]:
        print(f"  {tot:9.1f} ms  x{n:<4} max {mx:7.1f}  {what}")

# Rune's own split of each input (RUNE_PERF=1 through perf-collector.py):
# input to present, and the update's phases.
if len(sys.argv) > 2:
    import json, os
    if os.path.exists(sys.argv[2]):
        print("\n== Rune, per input (input to present; handler / view / commit / layout / text / paint / apply ms; nodes)")
        val = lambda v: next(iter(v.values()))
        for line in open(sys.argv[2]):
            d = json.loads(line)
            for rs in d.get("resourceSpans", []):
                for ss in rs["scopeSpans"]:
                    spans = ss["spans"]
                    for sp in spans:
                        if sp["name"] != "ui.interaction": continue
                        a = {x["key"]: val(x["value"]) for x in sp["attributes"]}
                        up = next((s for s in spans if s["name"] == "update" and s.get("traceId") == sp["traceId"]), None)
                        u = {x["key"]: float(val(x["value"])) for x in up["attributes"]} if up else {}
                        ph = " / ".join(f"{u.get('rune.' + k + '_ms', 0):.1f}" for k in ["handler", "view", "commit", "layout", "text", "paint", "apply"])
                        print(f"  {float(a.get('rune.input_to_present_ms', 0)):7.1f} ms  {a.get('rune.control', '?')} {a.get('rune.action', '')}: {ph}; {a.get('rune.nodes', '?')} nodes")
