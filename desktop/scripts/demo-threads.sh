#!/usr/bin/env bash
# Threads, plans and the lead, headless.
#
#   desktop/scripts/demo-threads.sh            # every fixture state, one PNG each (no daemon)
#   desktop/scripts/demo-threads.sh window     # the whole window against the stand-in, scripted
#
# Part 1: `screen.sh <screen> fixtures/threads/<fixture>.json` for each pair
# below. Out: $SBXW_SHOTS/threads/<screen>-<fixture>.png and the element tree.
#
# Part 2 (`window`): the whole window against the stand-in daemon with
# fixtures/threads/standin.json. The script opens web-app's main session,
# turns Team mode on from its pane bar; the lead's plan arrives (the
# stand-in proposes it 0.9 s later) and its dialog opens; the person gives
# the docs thread a repository (on its card) and approves through the core's dialog; the
# three threads open in panes; the Threads panel shows the sessions; the
# plan dialog's status reads the plan again (the stand-in moves it one step
# per read) until the lead proposes the merge; Merge, confirmed in the
# core's dialog, stops at a conflict on web-orders; Retry merges the rest.
# One PNG per step in $SBXW_SHOTS/threads/window-*.png, and the app log.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/threads"; mkdir -p "$shots"
fx="$here/fixtures/threads"

pairs=(
  threads:basic threads:empty threads:errors threads:board threads:worktrees threads:plan-pending threads:merge-proposed
  plans:plan-pending plans:plan-edited plans:plan-ceiling plans:plan-approved
  plans:merge-proposed plans:merge-running plans:merge-failed plans:merge-merged plans:merge-shared
  threads-new:new-thread threads-new:new-session threads-new:new-full
)

if [ "${1:-}" != "window" ]; then
  for p in "${pairs[@]}"; do
    screen="${p%%:*}"; f="${p##*:}"
    case "$screen" in threads) size="${SHOT_SIZE:-380x820}" ;; plans) size="${SHOT_SIZE:-800x720}" ;; *) size="${SHOT_SIZE:-560x620}" ;; esac
    "$here/scripts/screen.sh" "$screen" "$fx/$f.json" --size "$size" --out "$shots/$screen-$f.png" | grep -v '^/' || true
    echo "$shots/$screen-$f.png"
  done
  exit 0
fi

# ── Part 2: the whole window ──────────────────────────────────────────────
script="$shots/window-script.json"
python3 - >"$script" <<'PY'
import json
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.3
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
plan = "p-standin-1"
add(type="tap", key="row-web-app")
mark("main", 600)
add(type="tap", key="pane-1-x-lead")
mark("plan-arrived", 1600)
add(type="tap", key="pt-3-repo")
add(type="tap", key="pt-3-repo-=docs")
mark("edited", 300)
add(type="tap", key="plan-approve")
mark("approve-dialog", 500)
add(type="tap", key="cf-confirm")
mark("approved", 900)
add(type="tap", key="extra-sandbox-web-app-threads")
mark("panel", 600)
add(type="tap", key="plan-status-" + plan)
mark("status", 1300)
add(type="tap", key="plan-reload"); mark("_r1", 1300)
add(type="tap", key="plan-reload"); mark("_r2", 1300)
add(type="tap", key="plan-reload")
mark("merge-proposed", 800)
add(type="tap", key="merge-all")
mark("merge-dialog", 300)
add(type="tap", key="cf-confirm")
mark("merge-failed", 1500)
add(type="tap", key="merge-all")
add(type="tap", key="cf-confirm")
mark("merged", 1500)
mark("end", 300)
print(json.dumps({"events": ev}, indent=1))
PY
marks=(${MARKS:-main plan-arrived edited approve-dialog approved panel status merge-proposed merge-dialog merge-failed merged end})
build
for m in "${marks[@]}"; do
  start_daemon "$shots/standin-$m" --fixture "$fx/standin.json"
  SHOT_SIZE="${SHOT_SIZE:-1400x860}" run_app "$shots/window-$m.log" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/window-$m.png"
done
grep -hE "threads|confirmed|intent from" "$shots/window-end.log" | sed 's/^\[[^]]*\] //' | head -60 || true
