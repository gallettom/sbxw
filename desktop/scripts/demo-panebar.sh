#!/usr/bin/env bash
# The pane bars and the top bar when their extras do not fit, headless, from
# the fixtures fixtures/shell/panebar-{1,2,4}.json (test-sbx with a plan
# waiting, sbx's requests, agents' requests, Team, Esc, Remote, Threads,
# Browser, Project; the subscription usage high in the top bar). Each shot
# closes the plan dialog and the Requests panel the fixture opens (Escape,
# the panel's ×), then opens an overflow menu and waits for its list:
#   panebar-1400-1.png   one pane at 1400x900: everything inline
#   panebar-1400-2.png   two panes at 1400x900, pane 1's menu open
#   panebar-1400-4.png   four panes at 1400x900, pane 2's menu open
#   panebar-720-2.png    two panes at 720x480 (the smallest window), pane 1's menu open
#   panebar-720-4.png    four panes at 720x480, pane 4's menu open
#   panebar-720-top.png  four panes at 720x480, the top bar's menu open
# Each shot has its element tree beside it (.json). MARKS=<name> runs one.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
build
shot() { # name size fixture menu-key
  local name="$1" size="$2" fixture="$3" menu="$4"
  case " ${MARKS:-$name} " in *" $name "*) ;; *) return 0 ;; esac
  local script="$shots/script-$name.json" open=""
  [ -n "$menu" ] && open="{\"t\": 1.2, \"type\": \"mark\", \"name\": \"wait:$menu\"}, {\"t\": 1.3, \"type\": \"tap\", \"key\": \"$menu\"}, {\"t\": 1.4, \"type\": \"mark\", \"name\": \"wait:$menu-list\"},"
  cat >"$script" <<JSON
{"events": [
  {"t": 0.5, "type": "mark", "name": "wait:plan-x"},
  {"t": 0.7, "type": "key", "name": "Escape"},
  {"t": 0.8, "type": "mark", "name": "hold:300"},
  {"t": 0.9, "type": "tap", "key": "panel-close-relay"},
  {"t": 1.0, "type": "mark", "name": "hold:300"},
  $open
  {"t": 1.6, "type": "mark", "name": "end"}
]}
JSON
  SHOT_SIZE="$size" run_app "$shots/app-$name.log" --fixture "$here/fixtures/shell/$fixture.json" --script "$script" \
    --snap "$shots/$name.png" --at mark:end+0.5 --tree "$shots/$name.json"
  grep -hE "waited|gave up|script:" "$shots/app-$name.log" | sed 's/^\[[^]]*\] //' || true
  echo "$shots/$name.png"
}
shot panebar-1400-1 1400x900 panebar-1 ""
shot panebar-1400-2 1400x900 panebar-2 pane-1-more
shot panebar-1400-4 1400x900 panebar-4 pane-2-more
shot panebar-720-2 720x480 panebar-2 pane-1-more
shot panebar-720-4 720x480 panebar-4 pane-4-more
shot panebar-720-top 720x480 panebar-4 extra-topbar
