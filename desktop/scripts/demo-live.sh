#!/usr/bin/env bash
# Live updates, headless, against the stand-in daemon. One run:
#   t=0.3  tap the data-pipeline row: its agent opens in the pane, which
#          starts the stopped sandbox; the stand-in says so on /api/stream
#          and the list reads `running` without anyone asking it to;
#   t=1.0  tap Stop on docs: POST stop, then the list reads `stopped`;
#   t=1.5  nothing in the app: the stand-in flips web-api to `running` on its
#          own (`--change`), as `sbx` would behind the daemon; the stream
#          event makes the app read the list again.
# `hold:<ms>` marks make the app wait in real time (the virtual clock would
# not wait for the daemon). Output: $SBXW_SHOTS/live-strip.png (five frames 0.5 s
# apart from 0.2 s, sidebar and pane bar), $SBXW_SHOTS/live-end.png, the logs.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root"; mkdir -p "$shots"
script="$shots/live-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "tap", "key": "row-data-pipeline"},
  {"t": 0.5, "type": "mark", "name": "hold:1500"},
  {"t": 1.0, "type": "tap", "key": "stop-docs"},
  {"t": 1.5, "type": "mark", "name": "hold:6000"},
  {"t": 2.0, "type": "mark", "name": "end"}
]}
JSON
change=(--change "3.5:web-api=running")
start_daemon "$shots" "${change[@]}"
run_app "$shots/app-live-strip.log" --script "$script" --start 0.2 --frames 121 --every 30 --cols 5 \
  --strip "$shots/live-strip.png" --crop 0,0,760,470
cleanup; pids=()
start_daemon "$shots" "${change[@]}"
run_app "$shots/app-live-end.log" --script "$script" --snap "$shots/live-end.png" --at 2.2
echo "$shots/live-strip.png"
echo "$shots/live-end.png"
grep -hE "intent from|stop docs|hold|list: " "$shots/app-live-strip.log" | sed 's/^\[[^]]*\] //'
