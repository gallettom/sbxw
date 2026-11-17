#!/usr/bin/env bash
# Tiling by drag and drop, headless, from fixtures/shell/layout-pages.json
# (seven sandboxes, four per page). Each mid-drag shot is taken with the
# pointer still pressed, so the drop zone's preview shows:
#   tiling-<size>-split.png     pane 1 over pane 2's bottom half (split: landscape)
#   tiling-<size>-split-left.png pane 1 over pane 2's left half
#   tiling-<size>-swap.png      pane 1 over pane 4's middle (swap)
#   tiling-<size>-dock.png      pane 1 at the page's right edge (dock along it)
#   tiling-<size>-tab.png       pane 1 over page 2's tab (move there; 1284 only)
#   tiling-<size>-new.png       pane 1 over the pager's "+" (a new page)
#   tiling-<size>-three.png     3 per page, pane 1 dropped at the bottom edge: three wide rows
#   tiling-<size>-rotate.png    then Rotate split from the palette on the dropped pane: it turns from a wide row into a tall column beside the other two
#   tiling-<size>-menu.png      pane 1's menu open: its moves
# for <size> 1284x816 and 720x480, each with its element tree (.json).
# MARKS="<name> ..." runs some.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
build
fixture="$here/fixtures/shell/layout-pages.json"
# The fixture opens the Requests panel: closed first, so the page has the room.
pre='{"t": 0.4, "type": "mark", "name": "wait:panel-close-relay:1500"}, {"t": 0.5, "type": "tap", "key": "panel-close-relay"}, {"t": 0.7, "type": "mark", "name": "hold:200"}'
run() { # name size script-body
  local name="$1" size="$2" body="$3"
  case " ${MARKS:-$name} " in *" $name "*) ;; *) return 0 ;; esac
  printf '{"events": [%s, %s, {"t": 9.0, "type": "mark", "name": "end"}]}\n' "$pre" "$body" | python3 -c '
import json, sys
d = json.load(sys.stdin)
ev = sorted(d["events"], key=lambda e: e["t"])
# The end mark right after the last step.
last = max(e["t"] for e in ev if e.get("name") != "end")
for e in ev:
    if e.get("name") == "end": e["t"] = last + 0.05
json.dump({"events": ev}, sys.stdout)' >"$shots/script-$name.json"
  SHOT_SIZE="$size" run_app "$shots/app-$name.log" --fixture "$fixture" --script "$shots/script-$name.json" \
    --snap "$shots/$name.png" --at mark:end+0.15 --tree "$shots/$name.json"
  grep -hE "gave up|script:" "$shots/app-$name.log" | sed 's/^\[[^]]*\] //' || true
  echo "$shots/$name.png"
}
# A drag from pane 1's grip to a point given by a Python expression over the
# base tree's rects `r` (by key), pressed and not released.
drag_body() { # size from-key expr [start time] [release: 1]
  local size="$1" from="$2" expr="$3" t0="${4:-1.0}" up="${5:-}"
  python3 - "$shots/base-$size.json" "$from" "$expr" "$t0" "$up" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
r = {n["key"]: n["rect"] for n in t["elements"] if n.get("key")}
fx, fy = (lambda q: (q[0] + q[2] / 2, q[1] + q[3] / 2))(r[sys.argv[2]])
x, y = eval(sys.argv[3], {"r": r})
t0 = float(sys.argv[4])
ev = [{"t": t0, "type": "mark", "name": "wait:" + sys.argv[2]}, {"t": t0 + 0.1, "type": "down", "x": fx, "y": fy}]
for k in range(1, 9):
    f = k / 8
    ev.append({"t": t0 + 0.1 + 0.05 * k, "type": "move", "x": fx + (x - fx) * f, "y": fy + (y - fy) * f})
if sys.argv[5]:
    ev.append({"t": t0 + 0.6, "type": "up", "x": x, "y": y})
    ev.append({"t": t0 + 0.7, "type": "mark", "name": "hold:200"})
print(",".join(json.dumps(e) for e in ev))
PY
}
for size in 1284x816 720x480; do
  # The base layout, for the drop points.
  printf '{"events": [%s, {"t": 1.0, "type": "mark", "name": "end"}]}\n' "$pre" >"$shots/script-base-$size.json"
  SHOT_SIZE="$size" run_app "$shots/app-base-$size.log" --fixture "$fixture" --script "$shots/script-base-$size.json" \
    --snap "$shots/base-$size.png" --at mark:end+0.3 --tree "$shots/base-$size.json"
  run "tiling-$size-split" "$size" "$(drag_body "$size" pane-1-grip "(r['pane-2'][0] + r['pane-2'][2] / 2, r['pane-2'][1] + r['pane-2'][3] * 0.88)")"
  run "tiling-$size-split-left" "$size" "$(drag_body "$size" pane-1-grip "(r['pane-2'][0] + r['pane-2'][2] * 0.12, r['pane-2'][1] + r['pane-2'][3] / 2)")"
  run "tiling-$size-swap" "$size" "$(drag_body "$size" pane-1-grip "(r['pane-4'][0] + r['pane-4'][2] / 2, r['pane-4'][1] + r['pane-4'][3] / 2)")"
  run "tiling-$size-dock" "$size" "$(drag_body "$size" pane-1-grip "(r['page-area'][0] + r['page-area'][2] - 6, r['page-area'][1] + r['page-area'][3] / 2)")"
  # At 720 the other page's tab is in the pager's menu: no tab to drop on.
  [ "$size" = 720x480 ] || run "tiling-$size-tab" "$size" "$(drag_body "$size" pane-1-grip "(r['page-2'][0] + r['page-2'][2] / 2, r['page-2'][1] + r['page-2'][3] / 2)")"
  run "tiling-$size-new" "$size" "$(drag_body "$size" pane-1-grip "(r['page-new'][0] + 12, r['page-new'][1] + 12)")"
  # Three per page, then pane 1 docked along the bottom: 2 | 3 above, 1 wide below.
  three='{"t": 1.0, "type": "tap", "key": "pager-per"}, {"t": 1.2, "type": "tap", "key": "pager-per-3"}, {"t": 1.5, "type": "mark", "name": "hold:200"}, '"$(drag_body "$size" pane-1-grip "(r['page-area'][0] + r['page-area'][2] / 2, r['page-area'][1] + r['page-area'][3] - 6)" 1.6 1)"
  run "tiling-$size-three" "$size" "$three"
  # Rotate from the keyboard: the palette (⌘K / Ctrl+K), "rotate", Enter (the dropped pane has the focus).
  run "tiling-$size-rotate" "$size" "$three"', {"t": 2.6, "type": "key", "name": "k", "cmd": true}, {"t": 2.7, "type": "mark", "name": "wait:palette-input"}, {"t": 2.8, "type": "type", "text": "rotate", "every": 0.02}, {"t": 3.1, "type": "key", "name": "Enter"}, {"t": 3.3, "type": "mark", "name": "hold:200"}'
  run "tiling-$size-menu" "$size" '{"t": 1.0, "type": "tap", "key": "pane-1-more"}, {"t": 1.2, "type": "mark", "name": "wait:pane-1-more-list"}'
done
