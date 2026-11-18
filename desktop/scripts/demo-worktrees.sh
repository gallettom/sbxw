#!/usr/bin/env bash
# The Worktrees view (an overlay over the pane area), headless, against the
# stand-in daemon with fixtures/worktrees/standin.json (web-app's three
# worktrees, docs' one; thread states from its sessions):
#
#   desktop/scripts/demo-worktrees.sh
#   MARKS="list" SIZES=720x480 desktop/scripts/demo-worktrees.sh
#
# Marks: `list` (the rail's Worktrees, with the branch graph of the unmerged
# worktrees), `commits` (a row's Commits: the Commits view on that branch),
# `lane` (a lane's label pressed: its row marked), `dot` (a commit's dot
# pressed: Commits on that commit, opened). Out: $SBXW_SHOTS/worktrees/worktrees-<size>-<mark>.png
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/worktrees"; mkdir -p "$shots"
build
script() {
  MARK="$1" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="mark", name="wait:row-web-app")
step("rail-worktrees", "worktrees-row-web-app:checkout-copy")
if m == "commits":
    step("worktrees-commits-web-app:checkout-copy", "commits-branch")
if m == "lane":
    add(type="tap", key="worktrees-lane-web-app:api-cache")
if m == "dot":
    # checkout-copy's tip (the same hash in fixtures/commits/standin.json).
    step("worktrees-dot-web-app-b1b942e", "commits-branch")
add(type="mark", name="hold:400")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for size in ${SIZES:-1284x816 720x480}; do
  for m in ${MARKS:-list commits lane dot}; do
    script "$m" >"$shots/script-$m.json"
    start_daemon "$shots/standin-$size-$m" --fixture "$here/fixtures/worktrees/standin.json"
    SHOT_SIZE="$size" run_app "$shots/worktrees-$size-$m.log" --script "$shots/script-$m.json" \
      --snap "$shots/worktrees-$size-$m.png" --tree "$shots/worktrees-$size-$m.json" --at "mark:$m+0.2"
    cleanup; pids=()
    grep -hE "gave up|no element|script:" "$shots/worktrees-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
    echo "$shots/worktrees-$size-$m.png"
  done
done
