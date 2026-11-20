#!/usr/bin/env bash
# File paths in a terminal pane, headless, on the stand-in daemon (no sbx):
#
#   desktop/scripts/demo-ttylinks.sh             # SBXW_SHOTS=<dir> to choose where
#   THEME=graphite desktop/scripts/demo-ttylinks.sh
#
# web-app's shell prints fixtures/ttylinks/session.txt. With ⌘ (Ctrl) held, the
# path under the pointer is underlined (1-hover); a click opens the Files
# screen on it (2-files). Out: $SBXW_SHOTS/ttylinks-<theme>-<mark>.png and the tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
export SHOT_SIZE="${SHOT_SIZE:-1284x816}"
theme="${THEME:-dark}"
start_daemon "$shots" --fixture "$here/fixtures/files/standin.json"
show="clear; cat $here/fixtures/ttylinks/session.txt"
base() { cat <<JSON
  {"t": 0.3, "type": "paste", "text": "$show"},
  {"t": 0.6, "type": "key", "name": "Enter"},
  {"t": 1.2, "type": "mark", "name": "hold:400"}
JSON
}
# Pass 0: where the grid is (its rectangle), to aim at a cell.
cat >"$shots/tl-0.json" <<JSON
{"events": [ $(base), {"t": 2.0, "type": "mark", "name": "end"} ]}
JSON
run_app "$shots/app-tl-0.log" --sandbox web-app --script "$shots/tl-0.json" --snap "$shots/tl-0.png" --tree "$shots/tl-0-tree.json" --at mark:end+0.3
read -r X Y < <(python3 - "$shots/tl-0-tree.json" "$here" <<'PY'
import json, sys, re
tree = json.load(open(sys.argv[1]))
found = []
def walk(n):
    if isinstance(n, dict):
        if str(n.get("key", "")).startswith("grid"):
            found.append(n.get("rect") or n.get("bounds"))
        for v in n.values(): walk(v)
    elif isinstance(n, list):
        for v in n: walk(v)
walk(tree)
r = found[0]
cw, lh = 8.4, 18.0   # overridden by CW when set
import os
cw = float(os.environ.get("CW", cw))
# "  --> src/main.ts:42:7": the path starts at column 6, row 2.
print(r[0] + (6 + 4) * cw, r[1] + 2.5 * lh)
PY
)
cat >"$shots/tl-1.json" <<JSON
{"events": [ $(base),
  {"t": 1.5, "hold": "cmd"},
  {"t": 1.6, "type": "move", "x": $X, "y": $Y},
  {"t": 1.9, "type": "mark", "name": "hold:300"},
  {"t": 2.2, "type": "mark", "name": "hover"},
  {"t": 2.4, "type": "tap", "x": $X, "y": $Y},
  {"t": 2.5, "type": "mark", "name": "wait:overlay-files"},
  {"t": 2.6, "type": "mark", "name": "hold:2500"},
  {"t": 3.4, "type": "mark", "name": "opened"}
]}
JSON
run_app "$shots/app-tl-1.log" --sandbox web-app --script "$shots/tl-1.json" --snap "$shots/ttylinks-$theme-1-hover.png" --tree "$shots/ttylinks-$theme-1-hover.json" --at mark:hover+0.2
run_app "$shots/app-tl-2.log" --sandbox web-app --script "$shots/tl-1.json" --snap "$shots/ttylinks-$theme-2-files.png" --tree "$shots/ttylinks-$theme-2-files.json" --at mark:opened+0.3
echo "$shots/ttylinks-$theme-1-hover.png $shots/ttylinks-$theme-2-files.png"
