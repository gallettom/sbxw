#!/usr/bin/env bash
# The Network panel's sandbox filter, headless, against the stand-in daemon:
#
#   desktop/scripts/demo-netfilter.sh
#
# fixtures/network/filtered.json: 113 refused-host requests of 33 hosts in 9
# sandboxes, test-sbx's 2 among them, none for notes. Three runs, one shot
# each, at 1284x816:
#   bubble   press test-sbx's blue bubble: only its 2 requests, the chip
#            "test-sbx x", "Show all 113", "Dismiss these 2";
#   empty    select notes, open its Network item: "Nothing waits in notes.";
#   all      press the top bar's "sbx asks": every sandbox's, 25 cards drawn,
#            "Show 25 more", "Dismiss all 113".
# Out: $SBXW_SHOTS/netfilter/<mark>.png, the element tree and the app log beside each;
# then sessions-plus.png (below).
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/netfilter"; mkdir -p "$shots"
tap() { # <mark> <key> [<key tapped first>]
  local first=""
  [ -n "${3:-}" ] && first="{\"t\": 0.2, \"type\": \"tap\", \"key\": \"$3\"},"
  cat >"$shots/$1-script.json" <<JSON
{"events": [
  $first
  {"t": 0.3, "type": "mark", "name": "wait:$2:8000"},
  {"t": 0.5, "type": "tap", "key": "$2"},
  {"t": 0.7, "type": "mark", "name": "wait:net-sandbox:8000"},
  {"t": 0.9, "type": "mark", "name": "hold:600"},
  {"t": 1.1, "type": "mark", "name": "$1"}
]}
JSON
}
tap bubble extra-sandbox-test-sbx-approvals
tap empty extra-sandbox-notes-network extra-sandbox-notes
tap all extra-topbar-approvals
build
for m in ${MARKS:-bubble empty all}; do
  start_daemon "$shots/standin-$m" --fixture "$here/fixtures/network/filtered.json"
  SHOT_SIZE="${SHOT_SIZE:-1284x816}" run_app "$shots/$m.log" \
    --script "$shots/$m-script.json" --snap "$shots/$m.png" --tree "$shots/$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/$m.png"
  grep -hE "intent from|waited|gave up|no element" "$shots/$m.log" | sed 's/^\[[^]]*\] //' || true
done
# The sidebar's sessions block (fixtures/threads/sessions-plus.json): main in
# pane 1 (P1); s1 idle, t1 working, t2 wanting you, in no pane: a green +;
# old-fix done: none. Out: $SBXW_SHOTS/netfilter/sessions-plus.png.
SHOT_SIZE="${SHOT_SIZE:-1284x816}" run_app "$shots/sessions-plus.log" --fixture "$here/fixtures/threads/sessions-plus.json" \
  --snap "$shots/sessions-plus.png" --tree "$shots/sessions-plus.tree.json"
echo "$shots/sessions-plus.png"
# Vendor colours of the model words (fixtures/sandboxes/vendors.json): opus
# wanting you beside sonnet working, gpt-5-codex, o4-mini, gemini working, an
# unknown maker. Out: $SBXW_SHOTS/netfilter/vendors.png.
SHOT_SIZE="${SHOT_SIZE:-1284x816}" run_app "$shots/vendors.log" --fixture "$here/fixtures/sandboxes/vendors.json" \
  --snap "$shots/vendors.png" --tree "$shots/vendors.tree.json"
echo "$shots/vendors.png"
