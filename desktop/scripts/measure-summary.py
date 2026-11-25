#!/usr/bin/env python3
"""The measurement run's report (scripts/measure.sh), from the app's log.

The app, with SBXW_DESKTOP_PERF=counts, logs once a second what its counters
moved (`perf: counts t=<s> cpu_ms=<ms> polls=<n> ...`, src/perf.rs). The
script cuts the run into scenarios with `perf:begin <name>` and
`perf:end <name>` marks and asks for the memory report (`mem:<name>`) after
each. This prints, per scenario: its seconds, the app's CPU percent of one
core, each counter per second, the reads and renders by key, the memory
report, and what the script recorded beside it (footprint on macOS).

    measure-summary.py app.log [--meta meta.txt] [--side side.txt] [--out measure.txt]
    measure-summary.py --parse measure.txt     # reads a report back (JSON), exits 1 if it lacks a scenario's numbers
"""
import json, re, sys

# The counters in the order printed, with what each says (src/perf.rs).
COUNTERS = [
    ("polls", "App::poll calls (every frame Rune draws, and resting timer wakes)"),
    ("poll_us", "microseconds in App::poll"),
    ("views", "the core's element tree built (shell::view)"),
    ("view_us", "microseconds building it"),
    ("panel_builds", "a screen's element tree built from its last render"),
    ("pushes", "push_data"),
    ("push_us", "microseconds in push_data"),
    ("push_bytes", "bytes of data handed to the screens (estimate)"),
    ("pushes_from_terminals", "pushes that only a terminal's change asked for"),
    ("screen_data", "full builds of the screen data"),
    ("feat_ctx", "of which for a feature's handler"),
    ("renders", "Luau renders"),
    ("render_us", "microseconds in Luau renders"),
    ("rows_shown", "terminal rows built, panes on the current page"),
    ("rows_hidden", "terminal rows built, panes on hidden pages"),
    ("hidden_changes", "polls of a hidden pane that reported a change"),
    ("term_messages", "terminal socket messages taken (one UI wake each)"),
    ("blinks", "cursor blinks"),
    ("blinks_covered", "of which under a cover"),
    ("watcher_wakes", "screen file-watcher thread wake-ups"),
    ("events", "stream events"),
    ("fetches", "data keys read"),
    ("fetch_requests", "requests those reads made"),
]


def parse_log(path):
    """Scenarios (in order) with their per-second lines, memory blocks and layout lines."""
    scen, cur, mems, mem, layout, start_layout = [], None, {}, None, None, None
    for line in open(path, errors="replace"):
        m = re.search(r"perf: ---- begin (.+)$", line)
        if m:
            cur = {"name": m.group(1).strip(), "secs": [], "layout": layout}
            scen.append(cur)
            continue
        m = re.search(r"perf: ---- end (.+)$", line)
        if m:
            cur = None
            continue
        m = re.search(r"perf: layout (.*)$", line)
        if m:
            layout = m.group(1).strip()
            start_layout = start_layout or layout
            if cur is not None:
                cur["layout"] = layout
            continue
        m = re.search(r"perf: counts (.*)$", line)
        if m and cur is not None:
            kv = dict(p.split("=", 1) for p in m.group(1).split() if "=" in p)
            cur["secs"].append(kv)
            continue
        m = re.search(r"mem: ==== (.*)$", line)
        if m:
            mem = {"rows": [], "head": None, "sum": None}
            mems[m.group(1).strip()] = mem
            continue
        if mem is None:
            continue
        m = re.search(r"mem: (rss .*)$", line)
        if m and mem["head"] is None:
            mem["head"] = m.group(1).strip()
            continue
        m = re.search(r"mem: \| (.*?) \| (\S+) \| (.*)$", line)
        if m:
            mem["rows"].append((m.group(1), m.group(2), m.group(3).strip()))
            continue
        m = re.search(r"mem: (sum of owners .*)$", line)
        if m:
            mem["sum"] = m.group(1).strip()
    return scen, mems, start_layout


def side_notes(path):
    """What the script recorded beside the app, by scenario: `== <name>` then lines."""
    out, cur = {}, None
    if not path:
        return out
    try:
        lines = open(path, errors="replace").read().splitlines()
    except OSError:
        return out
    for l in lines:
        if l.startswith("== "):
            cur = out.setdefault(l[3:].strip(), [])
        elif cur is not None and l.strip():
            cur.append(l.rstrip())
    return out


def report(log, meta, side):
    scen, mems, start_layout = parse_log(log)
    notes = side_notes(side)
    o = []
    o.append("sbxw desktop: measurement run (scripts/measure.sh)")
    o.append("")
    if meta:
        o += [l.rstrip() for l in open(meta, errors="replace") if l.strip()]
    if start_layout:
        o.append(f"layout at start: {start_layout}")
    o.append("frames drawn: Rune's, not visible to the app; polls bound them from above, views from below.")
    o.append("")
    for s in scen:
        n = len(s["secs"])
        o.append(f"## {s['name']}")
        if s.get("layout"):
            o.append(f"layout: {s['layout']}")
        if n == 0:
            o.append("seconds: 0 (no counts line in this scenario: the counters are off?)")
            o.append("")
            continue
        tot = {}
        cpu = 0.0
        cpu_known = True
        for kv in s["secs"]:
            for k, v in kv.items():
                if k in ("t", "cpu_ms"):
                    continue
                tot[k] = tot.get(k, 0) + int(v)
            if kv.get("cpu_ms", "-") == "-":
                cpu_known = False
            else:
                cpu += float(kv["cpu_ms"])
        o.append(f"seconds: {n}")
        o.append(f"cpu: {cpu / n / 10:.1f} % of one core" if cpu_known else "cpu: not measured")
        o.append("counters per second (total over the scenario):")
        for k, what in COUNTERS:
            v = tot.get(k, 0)
            o.append(f"  {k} = {v / n:.1f} ({v})  {what}")
        for kind in ("render", "fetch", "event"):
            ks = sorted(((k, v) for k, v in tot.items() if k.startswith(kind + ".")), key=lambda x: -x[1])
            if ks:
                o.append(f"  {kind} by key: " + ", ".join(f"{k[len(kind) + 1:]} {v / n:.1f}/s" for k, v in ks[:12]) + (f", and {len(ks) - 12} more" if len(ks) > 12 else ""))
        m = mems.get(s["name"])
        if m:
            o.append("memory (MB):")
            if m["head"]:
                o.append(f"  {m['head']}")
            for name, mb, note in m["rows"]:
                o.append(f"  | {name} | {mb} |" + (f" {note}" if note else ""))
            if m["sum"]:
                o.append(f"  {m['sum']}")
        for l in notes.get(s["name"], []):
            o.append(f"  {l}")
        o.append("")
    o.append("Linux headless numbers (software renderer, virtual frame clock, no cursor blink) say only that the probes work, nothing of the Mac's cost.")
    return "\n".join(o) + "\n"


def parse_report(path):
    """The report read back: per scenario, its seconds, cpu and counters."""
    out, cur = {"scenarios": []}, None
    for l in open(path, errors="replace"):
        l = l.rstrip("\n")
        m = re.match(r"## (.+)$", l)
        if m:
            cur = {"name": m.group(1), "seconds": 0, "cpu": None, "counters": {}, "memory": {}}
            out["scenarios"].append(cur)
            continue
        if cur is None:
            continue
        m = re.match(r"seconds: (\d+)", l)
        if m:
            cur["seconds"] = int(m.group(1))
        m = re.match(r"cpu: ([\d.]+) %", l)
        if m:
            cur["cpu"] = float(m.group(1))
        m = re.match(r"  (\w+) = ([\d.]+) \((\d+)\)", l)
        if m:
            cur["counters"][m.group(1)] = {"per_s": float(m.group(2)), "total": int(m.group(3))}
        m = re.match(r"  \| (.*?) \| (\S+) \|", l)
        if m:
            cur["memory"][m.group(1).strip()] = None if m.group(2) == "-" else float(m.group(2))
    return out


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] == "--parse":
        r = parse_report(args[1])
        print(json.dumps(r, indent=1))
        bad = [s["name"] for s in r["scenarios"] if s["seconds"] == 0 or not s["counters"] or not s["memory"]]
        if not r["scenarios"] or bad:
            print(f"incomplete: {bad or 'no scenario'}", file=sys.stderr)
            sys.exit(1)
        sys.exit(0)
    log = args.pop(0)
    opt = {}
    while args:
        k = args.pop(0)
        opt[k] = args.pop(0)
    text = report(log, opt.get("--meta"), opt.get("--side"))
    if "--out" in opt:
        open(opt["--out"], "w").write(text)
    sys.stdout.write(text)
