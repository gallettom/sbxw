#!/usr/bin/env bash
# Pages of terminals and the pager, headless, from
# fixtures/shell/layout-pages.json (seven sandboxes open; four per page by
# default). Each shot has its element tree beside it (.json):
#   pages-<size>-four.png    4 per page: two pages, page 1 shown
#   pages-<size>-three.png   3 per page (the pager's menu): three pages
#   pages-<size>-menu.png    the "per page" menu open (1, 2, 3, 4, 6, Gather)
#   pages-<size>-tabs.png    Tabs (one terminal per page), page 3 shown by Ctrl+Tab twice
#   pages-<size>-page2.png   page 2 shown by its tab (at 720: from the pager's menu)
# for <size> 1284x816 and 720x480. MARKS="<name> ..." runs some.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
build
fixture="$here/fixtures/shell/layout-pages.json"
pre='{"t": 0.4, "type": "mark", "name": "wait:panel-close-relay:1500"}, {"t": 0.5, "type": "tap", "key": "panel-close-relay"}, {"t": 0.7, "type": "mark", "name": "hold:200"}'
run() { # name size events
  local name="$1" size="$2" body="$3"
  case " ${MARKS:-$name} " in *" $name "*) ;; *) return 0 ;; esac
  printf '{"events": [%s%s, {"t": 9.0, "type": "mark", "name": "end"}]}\n' "$pre" "${body:+, $body}" | python3 -c '
import json, sys
d = json.load(sys.stdin)
ev = sorted(d["events"], key=lambda e: e["t"])
last = max(e["t"] for e in ev if e.get("name") != "end")
for e in ev:
    if e.get("name") == "end": e["t"] = last + 0.1
json.dump({"events": ev}, sys.stdout)' >"$shots/script-$name.json"
  SHOT_SIZE="$size" run_app "$shots/app-$name.log" --fixture "$fixture" --script "$shots/script-$name.json" \
    --snap "$shots/$name.png" --at mark:end+0.4 --tree "$shots/$name.json"
  grep -hE "gave up|script:" "$shots/app-$name.log" | grep -v panel-close-relay | sed 's/^\[[^]]*\] //' || true
  echo "$shots/$name.png"
}
three='{"t": 1.0, "type": "tap", "key": "pager-per"}, {"t": 1.1, "type": "mark", "name": "wait:pager-per-3"}, {"t": 1.2, "type": "tap", "key": "pager-per-3"}, {"t": 1.4, "type": "mark", "name": "hold:200"}'
for size in 1284x816 720x480; do
  run "pages-$size-four" "$size" ""
  run "pages-$size-three" "$size" "$three"
  run "pages-$size-menu" "$size" '{"t": 1.0, "type": "tap", "key": "pager-per"}, {"t": 1.1, "type": "mark", "name": "wait:pager-per-list"}'
  run "pages-$size-tabs" "$size" '{"t": 1.0, "type": "tap", "key": "pager-view-tabs"}, {"t": 1.2, "type": "mark", "name": "hold:200"}, {"t": 1.3, "type": "key", "name": "Tab", "ctrl": true}, {"t": 1.4, "type": "key", "name": "Tab", "ctrl": true}, {"t": 1.5, "type": "mark", "name": "hold:200"}'
  # Page 2 by its tab; at 720 the tab is in the pager's "…" menu.
  if [ "$size" = 720x480 ]; then
    run "pages-$size-page2" "$size" '{"t": 1.0, "type": "tap", "key": "pager-more"}, {"t": 1.1, "type": "mark", "name": "wait:pager-more-list"}, {"t": 1.3, "type": "tap", "key": "page-2"}, {"t": 1.5, "type": "mark", "name": "hold:200"}'
  else
    run "pages-$size-page2" "$size" '{"t": 1.0, "type": "tap", "key": "page-2"}, {"t": 1.2, "type": "mark", "name": "hold:200"}'
  fi
done
