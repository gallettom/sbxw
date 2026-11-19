#!/usr/bin/env bash
# The Canvas (an overlay over the pane area), headless, on the fixtures
# fixtures/canvas/basic.json and empty.json (invented sandboxes and drawn
# mock screens; no daemon, no real project):
#
#   desktop/scripts/demo-canvas.sh                   # every mark, both sizes, both themes
#   MARKS="rounds" SIZES=720x480 THEMES=graphite desktop/scripts/demo-canvas.sh
#
# Marks, each its own run:
#   rounds   web-app's canvas "Checkout review": round 1 (eight shots), its
#            review sent (grey pins 1 to 3, a note), round 2 under it (the
#            same screens in the same columns), the draft in the side list
#   zoom     the draft's pin 4 chosen in the list: the canvas zooms to its
#            shot, the pin and its row marked
#   asset    the example picture (asset 1) chosen in the list
#   remarks  (720x480 only) the Remarks tab: the side list on a narrow window
#   send     Send remarks: the ONE core dialog (target, every remark in full,
#            the picture with its size, what happens)
#   picker   the canvas picker open: canvases with date, shots and size, New
#   delete   Delete this canvas: the core dialog naming what goes
#   empty    a sandbox with no canvas: what the view says, Ask the agent
# Out: $SBXW_SHOTS/canvas/canvas-<theme>-<size>-<mark>.png and its element tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/canvas"; mkdir -p "$shots"
build
script() { # mark size
  MARK="$1" SIZE="$2" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
narrow = int(os.environ["SIZE"].split("x")[0]) < 1000
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until=None):
    add(type="tap", key=key)
    if until: add(type="mark", name="wait:" + until)
add(type="mark", name="wait:row-web-app")
if narrow:
    # 480 pt tall: the rail's browse zone folds into More.
    step("rail-more"); add(type="mark", name="hold:400")
step("rail-canvas", "canvas-sandbox")
if m != "empty":
    add(type="mark", name="wait:canvas")
    add(type="mark", name="hold:2500")
if m in ("zoom", "asset"):
    if narrow:
        step("canvas-tab-remarks", "canvas-row-pin-4")
    step("canvas-row-pin-4" if m == "zoom" else "canvas-row-a-1")
    if narrow:
        step("canvas-tab-canvas", "canvas")
    add(type="mark", name="hold:1500")
if m == "remarks":
    step("canvas-tab-remarks", "canvas-row-pin-4")
if m == "send":
    if narrow:
        step("canvas-tab-remarks", "canvas-send")
    step("canvas-send")
if m == "picker":
    step("canvas-pick")
if m == "delete":
    step("canvas-delete"); add(type="mark", name="hold:300"); step("canvas-delete-this")
add(type="mark", name="hold:800")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 720x480}; do
    for m in ${MARKS:-rounds zoom asset remarks send picker delete empty}; do
      [ "$m" = remarks ] && [ "$size" != 720x480 ] && continue
      fx="$here/fixtures/canvas/basic.json"
      [ "$m" = empty ] && fx="$here/fixtures/canvas/empty.json"
      script "$m" "$size" >"$shots/script-$m.json"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/canvas-$theme-$size-$m.log" --fixture "$fx" --script "$shots/script-$m.json" \
        --snap "$shots/canvas-$theme-$size-$m.png" --tree "$shots/canvas-$theme-$size-$m.json" --at "mark:$m+0.2"
      grep -hE "gave up|no element|script:|render:" "$shots/canvas-$theme-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/canvas-$theme-$size-$m.png"
    done
  done
done
