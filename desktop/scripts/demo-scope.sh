#!/usr/bin/env bash
# The scope picker's list and the overlays' title band, headless, against
# the stand-in daemon with fixtures/scope/twenty.json (twenty sandboxes in
# nine groups, eight with worktrees) or fixtures/scope/no-worktrees.json:
#
#   desktop/scripts/demo-scope.sh
#   MARKS="ordered" SIZES=720x480 THEMES=graphite desktop/scripts/demo-scope.sh
#
# Marks: `ordered` (Worktrees' list: only the sandboxes with worktrees, the
# most first), `empty` (Worktrees' list when none has one), `scroll`
# (Commits' list of twenty, scrolling under its pinned filter and All),
# `band-worktrees`, `band-files`, `band-commits` (each title band, scoped to
# web-app; Commits on a branch, its second level).
# Out: $SBXW_SHOTS/scope/<mark>-<theme>-<size>.png
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/scope"; mkdir -p "$shots"
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
if m in ("ordered", "empty"):
    step("rail-worktrees", "worktrees-sandbox")
    add(type="mark", name="hold:600")
    step("worktrees-sandbox", "worktrees-sandbox-list")
elif m == "scroll":
    step("rail-commits", "commits-sandbox")
    step("commits-sandbox", "commits-sandbox-list")
elif m == "band-worktrees":
    step("rail-worktrees", "worktrees-sandbox")
    step("worktrees-sandbox", "worktrees-sandbox-list")
    add(type="tap", key="worktrees-sandbox-web-app")
elif m == "band-files":
    step("rail-files", "files-sandbox")
    step("files-sandbox", "files-sandbox-list")
    add(type="tap", key="files-sandbox-web-app")
elif m == "band-commits":
    step("rail-commits", "commits-sandbox")
    step("commits-sandbox", "commits-sandbox-list")
    add(type="tap", key="commits-sandbox-web-app")
add(type="mark", name="hold:400")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 720x480}; do
    for m in ${MARKS:-ordered empty scroll band-worktrees band-files band-commits}; do
      fx="$here/fixtures/scope/twenty.json"
      [ "$m" = empty ] && fx="$here/fixtures/scope/no-worktrees.json"
      name="$m-$theme-$size"
      script "$m" >"$shots/script-$m.json"
      start_daemon "$shots/standin-$name" --fixture "$fx"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/$name.log" --script "$shots/script-$m.json" \
        --snap "$shots/$name.png" --tree "$shots/$name.json" --at "mark:$m+0.2"
      cleanup; pids=()
      grep -hE "gave up|no element|script:" "$shots/$name.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/$name.png"
    done
  done
done
