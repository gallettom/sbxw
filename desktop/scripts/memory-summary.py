#!/usr/bin/env python3
"""The memory run's table, from the app's log (`mem:` lines, written at each
`mem:<label>` mark: shell/memory.rs). One column per mark, one row per owner,
in MB. With --ceiling MB and --leak MB, exits 1 when the resident size at the
"open" mark exceeds the ceiling, or when the heap grew by more than the leak
allowance across the open/close cycles (MEMORY.md, "The guard").

    memory-summary.py app.log [--ceiling MB] [--leak MB] [--json out.json]
"""
import json, re, sys

args = sys.argv[1:]
log = args.pop(0)
opt = {}
while args:
    k = args.pop(0)
    opt[k] = args.pop(0)

marks, cur = [], None
for line in open(log, errors="replace"):
    m = re.search(r"mem: ==== (.*)$", line)
    if m:
        cur = {"label": m.group(1).strip(), "rows": {}}
        marks.append(cur)
        continue
    if cur is None:
        continue
    m = re.search(r"mem: rss (\S+) MB \(peak (\S+) MB\), heap live (\S+) MB, heap peak (\S+) MB", line)
    if m:
        for k, v in zip(["RSS", "RSS peak", "heap live", "heap peak"], m.groups()):
            cur["rows"][k] = None if v == "-" else float(v)
        continue
    m = re.search(r"mem: rss after trim (\S+) MB", line)
    if m:
        cur["rows"]["RSS after trim"] = float(m.group(1)); continue
    m = re.search(r"mem: \| (.*?) \| (\S+) \| (.*)$", line)
    if m:
        # An indented name is a part of the line above it; "-": not readable.
        name = m.group(1)
        cur["rows"][name] = None if m.group(2) == "-" else float(m.group(2))
        if m.group(3).strip():
            cur["rows"].setdefault("_notes", {})[name] = m.group(3).strip()
        continue
    m = re.search(r"mem: sum of owners (\S+) MB, rest (\S+) MB, heap live (\S+) MB", line)
    if m:
        cur["sum"] = [None if v == "-" else float(v) for v in m.groups()]

if not marks:
    print("no mem: lines in", log, file=sys.stderr)
    sys.exit(2)

order = []
for mk in marks:
    for k in mk["rows"]:
        if k != "_notes" and k not in order:
            order.append(k)
labels = [mk["label"] for mk in marks]
w = max(len(k) for k in order)
print("| " + "owner".ljust(w) + " | " + " | ".join(labels) + " |")
print("|" + "-" * (w + 2) + "|" + "|".join("-" * (len(l) + 2) for l in labels) + "|")
for k in order:
    cells = []
    for mk in marks:
        v = mk["rows"].get(k)
        cells.append(("-" if v is None else f"{v:.1f}").rjust(len(mk["label"])))
    print("| " + k.ljust(w) + " | " + " | ".join(cells) + " |")
print()
# The terminals as they were at the "terminals" mark (else the last mark with any).
withterms = [mk for mk in marks if any(k.strip().startswith("terminal ") for k in mk["rows"].get("_notes", {}))]
pick = next((mk for mk in marks if mk["label"] == "terminals"), withterms[-1] if withterms else None)
if pick:
    print(f"terminals at \"{pick['label']}\":")
    for k, v in pick["rows"].get("_notes", {}).items():
        if k.strip().startswith("terminal "):
            print(f"  {k.strip()}: {v}")

if "--json" in opt:
    json.dump(marks, open(opt["--json"], "w"), indent=1)

fail = False
by = {mk["label"]: mk["rows"] for mk in marks}
if "--ceiling" in opt:
    ceil = float(opt["--ceiling"])
    peak = max((mk["rows"].get("RSS") or 0) for mk in marks)
    print(f"guard: highest RSS {peak:.1f} MB, ceiling {ceil:.0f} MB: {'FAIL' if peak > ceil else 'ok'}")
    fail |= peak > ceil
if "--heap-ceiling" in opt and any(mk["rows"].get("heap live") is not None for mk in marks):
    ceil = float(opt["--heap-ceiling"])
    peak = max((mk["rows"].get("heap live") or 0) for mk in marks)
    print(f"guard: highest heap {peak:.1f} MB, ceiling {ceil:.0f} MB: {'FAIL' if peak > ceil else 'ok'}")
    fail |= peak > ceil
if "--leak" in opt:
    allow = float(opt["--leak"])
    ends = [mk["label"] for mk in marks if mk["label"].startswith("cycles-") and mk["label"] != "cycles-0"]
    a, b = by.get("cycles-0"), (by.get(ends[-1]) if ends else None)
    if a and b and a.get("heap live") is not None:
        grew = b["heap live"] - a["heap live"]
        print(f"guard: heap grew {grew:.1f} MB over the open/close cycles ({ends[-1]}), allowance {allow:.0f} MB: {'FAIL' if grew > allow else 'ok'}")
        fail |= grew > allow
    else:
        print("guard: no cycles-0 / cycles-N marks with the heap counted: leak check skipped")
sys.exit(1 if fail else 0)
