#!/usr/bin/env bash
# The app's memory, headless, in a RELEASE build, against the stand-in daemon
# with realistic sizes (MEMORY.md in the root artifacts folder):
#   - 12 sandboxes, all running, with sessions, threads and plans
#     (scripts/perf-fixture.py: 200 commits, a 3,000-entry folder, 113
#     requests, the worktrees);
#   - 3 panes per page, 8 terminals over 3 pages, each fed a long run of
#     agent-like output (scripts/memory-output.py: colours, wide characters,
#     redraws; MEM_LINES lines each, 30,000 by default);
#   - every overlay and panel of the rail opened once, a commit and its diff
#     expanded, the 3,000-entry folder opened, pictures previewed;
#   - everything closed again, an idle period, then 200 open/close cycles of
#     an overlay, a panel and a picture preview.
# A `mem:<label>` mark writes the report in the log (shell/memory.rs):
# resident size (RSS), the heap's live bytes (SBXW_DESKTOP_MEM=1, a counting
# allocator) and the bytes each owner holds. memory-summary.py prints the
# table and checks the guard (MEMORY.md, "The guard"): the highest heap
# under MEM_HEAP_CEILING_MB (420, twice this run's 210), the highest RSS
# under MEM_CEILING_MB (1000; this sandbox's software renderer alone holds
# about 165), and the heap's growth over the cycles under MEM_LEAK_MB (10).
#
#   desktop/scripts/memory.sh                   # Linux or macOS, one command
#
# Out: $SBXW_SHOTS/memory/memory.log, table.md, marks.json; exits 1 when the
# guard fails. MEM_COUNT=0 runs without the counting allocator (RSS only).
set -euo pipefail
export SBXW_PROFILE=release
source "$(dirname "$0")/lib.sh"
out="$shots_root/memory"; mkdir -p "$out"
build
bin=${MEM_BIN_DIR:-$bin}
python3 "$here/scripts/perf-fixture.py" >"$out/big.json"
# Twelve sandboxes, all running (the person's dozen).
python3 - "$out/big.json" >"$out/fixture.json" <<'PY'
import copy, json, sys
d = json.load(open(sys.argv[1]))
sbs = d["daemon"]["sandboxes"]
while len(sbs) < 12:
    x = copy.deepcopy(sbs[0]); x["name"] = f"extra-{len(sbs)}"; sbs.append(x)
for s in sbs:
    s["status"] = "running"
d["app"] = {}
# Four screenshots in the artifacts folder (1440x900, noisy enough to weigh
# what a real one weighs), previewed in Files.
import struct, zlib, random
def png(w, h, seed):
    r = random.Random(seed)
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        base = bytes(((x * 7 + y * 3 + seed * 40) & 255) for x in range(w))
        noise = r.randbytes(w)
        for x in range(w):
            rows += bytes((base[x], noise[x] & 63, (base[x] + y) & 255))
    def chunk(t, b): return struct.pack(">I", len(b)) + t + b + struct.pack(">I", zlib.crc32(t + b) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b"")
dd = d["daemon"]
folder = dd["files_tree"]["/Users/me/web/.sbxw-artifacts"]["entries"]
for i in range(1, 5):
    b = png(1440, 900, i)
    dd["files_content"][f"/Users/me/web/.sbxw-artifacts/shot-{i}.png"] = {"$raw_hex": b.hex()}
    folder.append({"name": f"shot-{i}.png", "kind": "file", "link": False, "size": len(b), "modified": 1794199000 + i})
    dd["artifacts"]["entries"].insert(0, {"path": f"shot-{i}.png", "name": f"shot-{i}.png", "size": len(b), "modified": 1794199900 + i})
print(json.dumps(d))
PY
python3 - "$out/fixture.json" "$here/scripts/memory-output.py" "${MEM_LINES:-30000}" "${MEM_CYCLES:-200}" "${MEM_IDLE_MS:-20000}" >"$out/script.json" <<'PY'
import json, sys
fx = json.load(open(sys.argv[1]))
gen, lines, cycles, idle = sys.argv[2], int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5])
names = [s["name"] for s in fx["daemon"]["sandboxes"]]
ev = []; t = 0.5
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.1
def hold(ms): add(type="mark", name=f"hold:{ms}")
def wait(key, ms=10000): add(type="mark", name=f"wait:{key}:{ms}")
def mem(label): add(type="mark", name="mem:" + label)
wait("row-web-app"); hold(1500)
mem("start")
# Three panes per page.
add(type="tap", key="pager-per"); wait("pager-per-3", 3000); add(type="tap", key="pager-per-3"); hold(300)
# Eight terminals, each fed its output as soon as it opens.
# The relay's panel opens on its own (pending requests): closed first.
add(type="tap", key="panel-close-relay"); hold(300)
# The eight whose rows show at the window's size (the list does not scroll in a script).
# web-app last, found through the list's filter: the explorer and the
# commits open on the sandbox used last.
term = ["chat-research", "chat-release-notes", "extra-11", "sbxw-jarvis", "sbxw-desktop-design", "sbxw-voice", "better-checkout", "web-app"]
for i, n in enumerate(term):
    if n == "web-app":
        add(type="tap", key="filter"); add(type="type", text="web-app", every=0.02); t += 0.3; wait("add-web-app", 3000)
    add(type="tap", key="add-" + n); hold(1500)
    add(type="paste", text=f"clear; python3 {gen} {lines} {i + 1}"); add(type="key", name="Enter"); hold(300)
hold(int(lines / 30000 * 25000) + 5000)
mem("terminals")
add(type="tap", key="filter")
for _ in range(7):
    add(type="key", name="Backspace")
hold(300)
RAIL = ["rail-commits", "rail-files", "rail-worktrees", "rail-board", "rail-mcp", "rail-skills", "rail-kits", "rail-network", "rail-requests", "rail-leads", "jarvis-toggle", "rail-setup"]
PICTURES = [f"shot-{i}.png" for i in range(1, 5)]
# Every overlay and panel of the rail once (the keys come from the run's own
# tree: rail-*), each left open until the next.
for k in RAIL:
    add(type="tap", key=k); hold(1200)
    if k == "rail-commits":
        row = "web-app:" + __import__("hashlib").sha1(b"web-app-0").hexdigest()
        wait("commits-row-" + row, 5000)
        add(type="tap", key="commits-row-" + row); wait("commits-file-src/area0/File0.tsx", 5000)
        add(type="tap", key="commits-file-src/area0/File0.tsx"); hold(500)
    if k == "rail-files":
        # It opens on the artifacts: the pictures there, then the workspace's 3,000-entry folder.
        wait("files-row-report.md", 5000)
        for p in PICTURES:
            add(type="tap", key="files-row-" + p); hold(1000)
        add(type="tap", key="files-loc-/Users/me/web"); wait("files-row-/Users/me/web/big", 5000)
        add(type="tap", key="files-row-/Users/me/web/big"); hold(1500)
mem("everything open")
# Close: the overlay or panel open, then every pane.
add(type="key", name="Escape"); hold(300)
add(type="key", name="Escape"); hold(300)
for k in ["panel-close-relay", "panel-close-network", "jarvis-toggle", "rail-leads"]:
    add(type="tap", key=k); hold(400)
# Every page closed, first first: its terminals with it.
for i in range(3):
    add(type="tap", key="page-1-close"); hold(500)
hold(1500)
mem("closed")
hold(idle)
mem("idle")
# Open/close cycles: an overlay, a panel, and every tenth a terminal pane
# and the explorer with two pictures previewed. A few first, so what a first
# open makes once (a screen's VM) is not counted as growth.
def cycle(c):
    add(type="tap", key="rail-commits"); add(type="tap", key="rail-commits")
    add(type="tap", key="rail-requests"); add(type="tap", key="panel-close-relay")
    if c % 10 == 0:
        add(type="tap", key="add-chat-research"); hold(500)
        add(type="tap", key="page-1-close"); hold(200)
        add(type="tap", key="rail-files"); wait("files-loc-artifacts", 3000)
        add(type="tap", key="files-loc-artifacts"); wait("files-row-shot-1.png", 3000)
        add(type="tap", key="files-row-shot-1.png"); hold(400)
        add(type="tap", key="files-row-shot-2.png"); hold(400)
        add(type="tap", key="rail-files"); hold(200)
for c in range(3):
    cycle(c * 10)
hold(1500)
mem("cycles-0")
for c in range(cycles):
    cycle(c)
hold(1500)
mem(f"cycles-{cycles}")
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
start_daemon "$out/standin" --fixture "$out/fixture.json"
bin=${MEM_BIN_DIR:-$bin}
SBXW_DESKTOP_MEM=${MEM_COUNT:-1} SHOT_SIZE=${SHOT_SIZE:-1440x900} run_app "$out/memory.log" --script "$out/script.json" --snap "$out/end.png" --tree "$out/tree.json" --at "${MEM_AT:-mark:end+0.2}" || true
cleanup; pids=()
python3 "$here/scripts/memory-summary.py" "$out/memory.log" --json "$out/marks.json" --ceiling "${MEM_CEILING_MB:-1000}" --heap-ceiling "${MEM_HEAP_CEILING_MB:-420}" --leak "${MEM_LEAK_MB:-10}" | tee "$out/table.md"
