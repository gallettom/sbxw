#!/usr/bin/env bash
# The Kanban's new-ticket form and the select lists of feature dialogs,
# headless, on invented fixtures:
#
#   desktop/scripts/demo-ticketform.sh
#   MARKS="rest list" SIZES=720x480 THEMES=graphite desktop/scripts/demo-ticketform.sh
#
# Marks: rest (the form at rest), list (the priority list open), long (a long
# description), files (two files attached), create-list (the create dialog's
# host list open), plans-list (the plan dialog's variant list open).
# Out: $SBXW_SHOTS/ticketform-<theme>-<size>-<mark>.png (SBXW_SHOTS defaults to the checkout's .sbxw-artifacts/desktop/shots).
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots/drop"
printf 'Invented notes for the ticket.\n' >"$shots/drop/members-notes.txt"
printf 'Invented second file.\n' >"$shots/drop/second.txt"
build
script() { # mark
  MARK="$1" SHOTS="$shots" python3 - <<'PY'
import json, os
m = os.environ["MARK"]; S = os.environ["SHOTS"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
if m in ("create-list",):
    add(type="mark", name="wait:create-host")
    step("create-host", "create-host-list")
elif m == "plans-list":
    add(type="mark", name="wait:plan-variant")
    step("plan-variant", "plan-variant-list")
else:
    add(type="mark", name="wait:row-web-app")
    step("rail-board", "board-convo")
    step("board-view-kanban", "kanban-lane-planned")
    step("kanban-new", "kanban-form")
    if m == "long":
        add(type="tap", key="kanban-form-description")
        add(type="type", text=("The members list should export as CSV with the filter applied. " * 12), every=0.002); t += 1.0
    if m == "files":
        ev.append({"drop_files": {"paths": [f"{S}/drop/members-notes.txt", f"{S}/drop/second.txt"], "key": "kanban-form", "hover": 0.2}, "t": round(t, 2)}); t += 0.5
        add(type="mark", name="wait:kanban-form-file-1")
    if m == "list":
        step("kanban-form-priority", "kanban-form-priority-list")
add(type="mark", name="hold:600")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 720x480}; do
    for m in ${MARKS:-rest list long files create-list plans-list}; do
      fx="$here/fixtures/kanban/basic.json"
      if [ "$m" = create-list ]; then # the fixture, with the create dialog open on it
        fx="$shots/create-open.json"
        python3 -c "import json,sys; d=json.load(open(sys.argv[1])); d.setdefault('app',{})['modal']=['create','remote']; json.dump(d,open(sys.argv[2],'w'))" "$here/fixtures/create/workspace-remote.json" "$fx"
      fi
      [ "$m" = plans-list ] && fx="$here/fixtures/threads/plan-pending.json"
      script "$m" >"$shots/script-$m.json"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/ticketform-$theme-$size-$m.log" --fixture "$fx" --script "$shots/script-$m.json" \
        --snap "$shots/ticketform-$theme-$size-$m.png" --tree "$shots/ticketform-$theme-$size-$m.json" --at "mark:$m+0.2" || echo "app failed on $m"
      grep -hE "gave up|no element|script:|render:" "$shots/ticketform-$theme-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/ticketform-$theme-$size-$m.png"
    done
  done
done
