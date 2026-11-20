#!/usr/bin/env bash
# The Kanban (the Board's second view, an overlay over the pane area),
# headless, on the fixture fixtures/kanban/basic.json (invented; no daemon):
#
#   desktop/scripts/demo-kanban.sh                    # every mark, both sizes, both themes
#   MARKS="board" SIZES=1284x816 THEMES=graphite desktop/scripts/demo-kanban.sh
#   FX=desktop/fixtures/kanban/big.json MARKS=board desktop/scripts/demo-kanban.sh
#
# Marks, each its own run:
#   board    web-app's board: cards in every column (a P1 with files, a
#            blocked thread, a failed thread with no ticket, one in test, two
#            waiting for the merge, merged ones, one validated)
#   all      every sandbox, each card with its sandbox's colour mark
#   ticket   T-5 open as a panel over the board: description, files, its
#            thread with its links, its history, Send to the lead disabled
#   planned  T-1 open: P1, two files, Move up and down, Send to the lead
#   form     the new-ticket form with two files dropped on it (one named
#            like a secret, marked "look first")
#   form-ack Save pressed on that form: the core's Confirm with the ack box
#   narrow   the window at 720 pt wide: one column at a time, its picker
# Out: $SBXW_SHOTS/kanban/kanban-<theme>-<size>-<mark>.png and its element tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/kanban"; mkdir -p "$shots" "$shots/drop"
fx="${FX:-$here/fixtures/kanban/basic.json}"
printf 'Invented notes for the ticket.\n' >"$shots/drop/members-notes.txt"
printf 'not a key\n' >"$shots/drop/id_rsa"
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
    if isinstance(key, dict):
        e = dict(key); e["t"] = round(t, 2); ev.append(e)
        globals()["t"] += 0.25
    else:
        add(type="tap", key=key)
    add(type="mark", name="wait:" + until)
add(type="mark", name="wait:row-web-app")
step("rail-board", "board-convo")
step("board-view-kanban", "kanban-lane-planned")
if m == "all":
    step("kanban-sandbox", "kanban-sandbox-list")
    step("kanban-sandbox-", "kanban-lane-planned")
if m == "ticket":
    step("kanban-row-t-web-app-5", "kanban-panel")
if m == "planned":
    step("kanban-row-t-web-app-1", "kanban-panel")
if m in ("form", "form-ack"):
    step("kanban-new", "kanban-form")
    add(type="tap", key="kanban-form-title"); add(type="type", text="Export the members list as CSV", every=0.01); t += 0.6
    step({"drop_files": {"paths": [f"{S}/drop/members-notes.txt", f"{S}/drop/id_rsa"], "key": "kanban-form", "hover": 0.2}}, "kanban-form-file-1")
if m == "form-ack":
    step("kanban-form-save", "cf-confirm")
if m == "narrow":
    step("kanban-colpick", "kanban-colpick-list")
    step("kanban-colpick-progress", "kanban-lane-progress")
add(type="mark", name="hold:600")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 1900x950 720x600}; do
    for m in ${MARKS:-board all ticket planned form form-ack narrow}; do
      [ "$m" = narrow ] && [ "$size" != 720x600 ] && continue
      [ "$m" != narrow ] && [ "$size" = 720x600 ] && continue
      script "$m" >"$shots/script-$m.json"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/kanban-$theme-$size-$m.log" --fixture "$fx" --script "$shots/script-$m.json" \
        --snap "$shots/kanban-$theme-$size-$m.png" --tree "$shots/kanban-$theme-$size-$m.json" --at "mark:$m+0.2"
      grep -hE "gave up|no element|script:|render:" "$shots/kanban-$theme-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/kanban-$theme-$size-$m.png"
    done
  done
done
