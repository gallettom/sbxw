#!/usr/bin/env bash
# Where an input's time goes, headless, in a RELEASE build, against the
# stand-in daemon with realistic sizes (scripts/perf-fixture.py: 11
# sandboxes, 200 commits with their details, a 3,000-entry folder, 113
# network requests, a 400-item conversation in Sessions, the Board's
# 500-event team and All teams over 26 teams with texts at the cap, the
# Kanban's 60 tickets and 40 threads). Each step of the script is announced in the log with a
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
# The first sandbox of the list: web-app's row is below the fold at 816 pt
# (11 sandboxes), and a row's menu shows on its selected or hovered row.
step("select a sandbox (tap its row)", "row-chat-research")
step("open a menu (a sandbox row's)", "extra-sandbox-chat-research", "extra-sandbox-chat-research-list")
add(type="key", name="Escape"); add(type="mark", name="hold:200")
# Sessions (GUI) as the base view, with an overlay over it, then back to the terminals (TUI).
step("make the Monitor the base view", "top-view-gui", "overlay-monitor")
# Sessions: web-app's 400-item conversation (perf-fixture.py) and its last group of steps.
step("Sessions: the 400-item conversation", "monitor-row-web-app:s-perf", "monitor-conv-web-app:s-perf")
step("Sessions: open its last group of steps", "monitor-steps-g394")
# (Show earlier is at the top of a scroll that opens at its end: a headless
# tap does not reach it; the 400 rows drawn are timed in tests.rs.)
step("open Commits over the Monitor", "rail-commits", "commits-row-" + row)
step("expand a commit over the Monitor", "commits-row-" + row, "commits-file-src/area0/File0.tsx")
add(type="mark", name="perf:close Commits: back to the Monitor"); add(type="key", name="Escape"); add(type="mark", name="wait-gone:overlay-commits"); add(type="mark", name="hold:300")
add(type="mark", name="perf:back to the terminals"); add(type="tap", key="top-view-tui"); add(type="mark", name="wait-gone:overlay-monitor"); add(type="mark", name="hold:300")
step("open the Commits overlay", "rail-commits", "commits-row-" + row)
step("expand a commit", "commits-row-" + row, "commits-file-src/area0/File0.tsx")
step("expand its first file's diff", "commits-file-src/area0/File0.tsx", "commits-diff-src/area0/File0.tsx")
step("fold the commit", "commits-row-" + row)
# The scope picker (scope.rs): its list with the filter (11 sandboxes), a pick, then All again.
step("Commits: open the scope picker", "commits-sandbox", "commits-sandbox-list")
step("Commits: pick a sandbox", "commits-sandbox-web-app", "commits-row-" + row)
step("Commits: All sandboxes again", "commits-sandbox", "commits-sandbox-list")
step("Commits: pick All", "commits-sandbox-", "commits-row-" + row)
add(type="mark", name="perf:type 6 characters in the filter")
add(type="tap", key="commits-filter"); add(type="type", text="change", every=0.05); t += 0.5
add(type="mark", name="hold:300")
step("open the Board (8 threads, 500 events)", "rail-board", "board-convo")
step("the Board: open the team picker", "board-row-picker", "board-row-team-p-perf")
step("the Board: its team", "board-row-team-p-perf", "board-more")
step("the Board: show earlier", "board-more")
step("the Board: decisions and warnings", "board-group-notes")
step("the Board: every kind again", "board-group-all")
step("the Board: the Commits counter (lanes, full width)", "board-count-commits", "board-lanes")
step("the Board: the counter again (every kind)", "board-count-commits", "board-convo")
step("the Board: the summary's details", "board-summary-details", "board-detail-part-1")
# The person's sizes (perf-fixture.py): a team of 500 events with texts at
# the 2000-character cap, then All teams (26 more teams).
step("the Board (real sizes): open the team picker", "board-row-picker", "board-row-team-p-huge")
step("the Board (real sizes): a team of 500 events at the cap", "board-row-team-p-huge", "board-fold-e0-499")
step("the Board (real sizes): Show more", "board-fold-e0-499")
step("the Board (real sizes): show earlier", "board-more")
step("the Board (real sizes): Messages", "board-group-messages")
step("the Board (real sizes): open the team picker again", "board-row-picker", "board-row-team-all")
step("the Board (real sizes): All teams", "board-row-team-all", "board-convo")
add(type="mark", name="perf:the Board (real sizes): All teams, type 3 characters in the search")
add(type="tap", key="board-search"); add(type="type", text="the", every=0.05); t += 0.3
add(type="mark", name="hold:300")
# (A button last: the search field gives up the keyboard before the Canvas.)
step("the Board (real sizes): All teams, Decisions and warnings", "board-group-notes")
step("the Board (real sizes): All teams, every kind", "board-group-all")
# The Kanban at real sizes (fixtures/kanban/big.json: 60 tickets, 40 threads over 25 plans).
step("the Kanban: the switch from the Board", "board-view-kanban", "kanban-lane-planned")
step("the Kanban: open the scope picker", "kanban-sandbox", "kanban-sandbox-list")
# (Beside the Requests panel the overlay is under 900 pt: one column at a time.)
step("the Kanban: All sandboxes", "kanban-sandbox-", "kanban-colpick")
step("the Kanban: the column picker", "kanban-colpick", "kanban-colpick-list")
step("the Kanban: Merged", "kanban-colpick-merged", "kanban-lane-merged")
step("the Kanban: Show older (Merged in full)", "kanban-older", "kanban-lane-merged")
step("the Kanban: the column picker again", "kanban-colpick", "kanban-colpick-list")
step("the Kanban: Planned", "kanban-colpick-planned", "kanban-lane-planned")
step("the Kanban: open the scope picker again", "kanban-sandbox", "kanban-sandbox-list")
add(type="type", text="web-app", every=0.03); t += 0.3; add(type="mark", name="hold:300")
step("the Kanban: web-app", "kanban-sandbox-web-app", "kanban-row-t-web-app-1")
step("the Kanban: open a ticket (its panel)", "kanban-row-t-web-app-1", "kanban-panel")
step("the Kanban: close it", "kanban-close")
step("the Kanban: back to the conversation", "kanban-view-conversation", "board-convo")
# Canvas: web-app's canvas of 15 shots, a pan and a pinch, the tools, Fit, then closed.
step("open the Canvas", "rail-canvas", "canvas-sandbox")
step("the Canvas: the sandbox picker", "canvas-sandbox", "canvas-sandbox-list")
# The scope list is bounded and scrolls (11 sandboxes): web-app is below its
# fold, so the filter (which has the keyboard) narrows it first, as a person would.
add(type="type", text="web-app", every=0.03); t += 0.3; add(type="mark", name="hold:300")
step("the Canvas: web-app's 15 shots", "canvas-sandbox-web-app", "canvas")
add(type="mark", name="hold:1500")
add(type="mark", name="perf:the Canvas: pan (wheel)"); add(type="wheel", x=560, y=450, dx=-40, dy=-30); add(type="mark", name="hold:300")
add(type="mark", name="perf:the Canvas: pinch"); add(type="magnify", x=560, y=450, delta=0.6); add(type="mark", name="hold:300")
step("the Canvas: the Pin tool", "canvas-tool-pin")
step("the Canvas: back to Select (Escape would leave the tool first)", "canvas-tool-select")
step("the Canvas: the Add example tool (its hint)", "canvas-tool-asset", "canvas-asset-hint")
step("the Canvas: back to Select", "canvas-tool-select")
step("the Canvas: fit", "canvas-zoom-fit")
add(type="mark", name="perf:the Canvas: closed"); add(type="key", name="Escape"); add(type="mark", name="wait-gone:overlay-canvas"); add(type="mark", name="hold:300")
# Beside the Requests panel the explorer is narrow: its locations are a menu.
step("switch to the Files overlay", "rail-files", "files-locations")
step("the locations", "files-locations", "files-locations-/Users/me/web")
step("the Workspace location", "files-locations-/Users/me/web", "files-up")
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
# A step that gave up waiting measured nothing: the run fails.
if grep -q "gave up waiting" "$out/perf.log"; then grep "gave up waiting" "$out/perf.log" >&2; exit 1; fi
