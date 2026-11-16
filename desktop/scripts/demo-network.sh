#!/usr/bin/env bash
# The network area, headless, against the stand-in daemon. One run:
#   t=0.3  tap web-app's network button in the sidebar: the Network panel
#          opens on web-app's policy and ports, nothing is waiting;
#   then   the stand-in makes sbx ask (`--approval 1`: web-app tried to reach
#          api.github.com); the app sees it on its own (a `session` event,
#          and the 5 s read): a notice, a badge on the row and in the top
#          bar, the request in the panel;
#   t=1.5  tap "Allow always": the CORE's dialog opens with the request's own
#          words (who, what, the option, how long); nothing is sent yet;
#   t=2.0  tap Confirm in that dialog: POST /api/approvals/respond; the list
#          empties and the host shows as an allow rule of web-app.
# Output: $SBXW_SHOTS/network-strip.png (six frames), $SBXW_SHOTS/network-end.png, logs.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
script="$shots/network-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "tap", "key": "extra-sandbox-web-app-network"},
  {"t": 0.5, "type": "mark", "name": "hold:2500"},
  {"t": 1.5, "type": "tap", "key": "answer-network:demo-allow"},
  {"t": 1.7, "type": "mark", "name": "hold:300"},
  {"t": 2.0, "type": "tap", "key": "cf-confirm"},
  {"t": 2.2, "type": "mark", "name": "hold:1500"},
  {"t": 2.6, "type": "mark", "name": "end"}
]}
JSON
args=(--fixture "$here/fixtures/network/empty.json" --approval 1)
start_daemon "$shots" "${args[@]}"
run_app "$shots/app-network-strip.log" --script "$script" --start 0.2 --frames 151 --every 30 --cols 6 \
  --strip "$shots/network-strip.png"
cp "$shots/standin.log" "$shots/standin-network.log"
cleanup; pids=()
start_daemon "$shots" "${args[@]}"
run_app "$shots/app-network-end.log" --script "$script" --snap "$shots/network-end.png" --at 2.7
echo "$shots/network-strip.png"
echo "$shots/network-end.png"
grep -hE "intent from|network:|confirmed|approval|hold" "$shots/app-network-strip.log" | sed 's/^\[[^]]*\] //'
grep -h "standin:" "$shots/standin-network.log" | grep -v "terminal" | head -20
