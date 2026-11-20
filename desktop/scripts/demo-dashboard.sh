#!/usr/bin/env bash
# The Dashboard (the feature `board`) and the top bar's Kanban button,
# headless, on invented fixtures (no daemon):
#
#   desktop/scripts/demo-dashboard.sh                 # every mark, both sizes, both themes
#   MARKS="lit" SIZES=720x480 THEMES=graphite desktop/scripts/demo-dashboard.sh
#
# Marks, each its own run:
#   rest     the top bar with the Kanban button at rest (kanban/basic.json)
#   lit      the button pressed: the Kanban drawn, titled "Dashboard", the button lit
#   convo    the rail's Dashboard: the conversation, titled "Dashboard" (board/basic.json)
#   tooltip  the pointer on the rail's Dashboard entry: its tooltip
# Out: $SBXW_SHOTS/dashboard/dashboard-<theme>-<size>-<mark>.png and its element tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/dashboard"; mkdir -p "$shots"
build
script() { # mark tree-of-rest
  MARK="$1" TREE="${2:-}" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
add(type="mark", name="wait:row-web-app")
add(type="mark", name="hold:400")
if m == "lit":
    add(type="tap", key="top-kanban"); add(type="mark", name="wait:kanban-lane-planned")
    # The pointer off the button: its tooltip would cover the title.
    add(type="move", x=700, y=500); add(type="mark", name="hold:800")
if m == "convo":
    add(type="tap", key="rail-board"); add(type="mark", name="wait:board-convo")
if m == "tooltip":
    # The entry's centre, from the element tree of the same window at rest.
    def find(n):
        if isinstance(n, dict):
            if n.get("key") == "rail-board":
                return n
            for v in n.values():
                r = find(v)
                if r: return r
        if isinstance(n, list):
            for v in n:
                r = find(v)
                if r: return r
    e = find(json.load(open(os.environ["TREE"])))
    r = e["bounds"]
    add(type="move", x=r[0] + r[2] / 2, y=r[1] + r[3] / 2)
    add(type="mark", name="hold:1500")
add(type="mark", name="hold:600")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 720x480}; do
    for m in ${MARKS:-rest lit convo tooltip}; do
      case "$m" in convo | tooltip) fx="$here/fixtures/board/basic.json" ;; *) fx="$here/fixtures/kanban/basic.json" ;; esac
      tree=""
      if [ "$m" = tooltip ]; then
        script rest >"$shots/script-at-rest.json"
        THEME="$theme" SHOT_SIZE="$size" run_app "$shots/at-rest.log" --fixture "$fx" --script "$shots/script-at-rest.json" \
          --tree "$shots/at-rest.json" --at "mark:rest+0.2"
        tree="$shots/at-rest.json"
      fi
      script "$m" "$tree" >"$shots/script-$m.json"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/dashboard-$theme-$size-$m.log" --fixture "$fx" --script "$shots/script-$m.json" \
        --snap "$shots/dashboard-$theme-$size-$m.png" --tree "$shots/dashboard-$theme-$size-$m.json" --at "mark:$m+0.2"
      grep -hE "gave up|no element|script:|render:" "$shots/dashboard-$theme-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/dashboard-$theme-$size-$m.png"
    done
  done
done
