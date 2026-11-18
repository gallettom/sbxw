#!/usr/bin/env bash
# Where an input's time goes, headless, in a RELEASE build, against the
# stand-in daemon with realistic sizes (scripts/perf-fixture.py: 11
# sandboxes, 200 commits with their details, a 3,000-entry folder, 113
# network requests). Each step of the script is announced in the log with a
# `perf:` mark; SBXW_DESKTOP_PERF=1 logs every Luau render, the data handed
# to the screens, the core's view and each daemon request with its time.
#
#   desktop/scripts/perf.sh            # runs, then prints the summary per step
#
# Out: $SBXW_SHOTS/perf/perf.log (the whole log) and the summary on stdout.
set -euo pipefail
export SBXW_PROFILE=release
source "$(dirname "$0")/lib.sh"
out="$shots_root/perf"; mkdir -p "$out"
build
# PERF_BIN_DIR: another build's sbxw and standin (an earlier commit's, to compare).
bin=${PERF_BIN_DIR:-$bin}
python3 "$here/scripts/perf-fixture.py" >"$out/big.json"
row="web-app:$(python3 -c 'import hashlib; print(hashlib.sha1(b"web-app-0").hexdigest())')"
python3 - "$row" >"$out/script.json" <<'PY'
import json, sys
row = sys.argv[1]
ev = []; t = 0.5
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.2
def step(label, key, until=None):
    add(type="mark", name="perf:" + label)
    add(type="tap", key=key)
    if until: add(type="mark", name="wait:" + until)
    add(type="mark", name="hold:300")
add(type="mark", name="wait:row-web-app")
add(type="mark", name="hold:500")
add(type="mark", name="perf:idle (a pointer move over the panes)"); add(type="move", x=700, y=500); add(type="mark", name="hold:300")
step("select a sandbox (tap its row)", "row-web-app")
step("open a menu (a sandbox row's)", "extra-sandbox-web-app", "extra-sandbox-web-app-list")
add(type="key", name="Escape"); add(type="mark", name="hold:200")
step("open the Commits overlay", "rail-commits", "commits-row-" + row)
step("expand a commit", "commits-row-" + row, "commits-file-src/area0/File0.tsx")
step("expand its first file's diff", "commits-file-src/area0/File0.tsx", "commits-diff-src/area0/File0.tsx")
step("fold the commit", "commits-row-" + row)
add(type="mark", name="perf:type 6 characters in the filter")
add(type="tap", key="commits-filter"); add(type="type", text="change", every=0.05); t += 0.5
add(type="mark", name="hold:300")
step("switch to the Files overlay", "rail-files", "files-tab-files")
step("the Workspace tab", "files-tab-files", "files-up")
step("browse the workspace (the folder above)", "files-up", "files-row-/Users/me/web/big")
step("open the 3,000-entry folder", "files-row-/Users/me/web/big")
add(type="mark", name="hold:800")
add(type="mark", name="browsed")
step("open the 3,000-entry folder", "files-row-/Users/me/web/big")
add(type="mark", name="hold:800")
add(type="mark", name="hold:800")
step("switch back to Commits", "rail-commits", "commits-filter")
add(type="mark", name="perf:hover the sidebar")
for i in range(10):
    add(type="move", x=120, y=150 + i * 30)
add(type="mark", name="hold:300")
add(type="mark", name="perf:end")
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
start_daemon "$out/standin" --fixture "$out/big.json"
bin=${PERF_BIN_DIR:-$bin}
# Rune's own phases (view, commit, layout, text, paint, apply) per input,
# through a local collector: every interaction is sent (threshold 0).
rm -f "$out/rune.jsonl"
python3 "$here/scripts/perf-collector.py" "$out/rune.jsonl" & pids+=($!)
sleep 0.3
RUNE_PERF=1 RUNE_PERF_SLOW_MS=0.01 SBXW_DESKTOP_PERF=1 SHOT_SIZE=1284x816 run_app "$out/perf.log" --script "$out/script.json" --snap "$out/perf.png" --tree "$out/tree.json" --at "${PERF_AT:-mark:end+0.2}" || true
cleanup; pids=()
python3 "$here/scripts/perf-summary.py" "$out/perf.log" "$out/rune.jsonl"   # exits 1 over the budget
