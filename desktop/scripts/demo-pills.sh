#!/usr/bin/env bash
# Keyword pills in a terminal, headless, on the stand-in daemon: a short
# agent reply with labels (fixtures/emphasis/reply.txt) printed in the
# web-app sandbox's shell. One shot per state, in $SBXW_SHOTS/emphasis/:
#   pills-1-terminal.png    every tone as a pill in the reply
#   pills-2-selection.png   a selection across two pills: they yield to it
#   pills-3-off.png         the daemon's switch turned off live (no restart)
#   pills-4-light.png       a light terminal theme (#fdf6e3), in term-demo
#   pills-5-scaled.png      another window owns a larger size: the scaled view
# Each with its element tree (.json) beside it. SHOT_SIZE defaults to 1284x816.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/emphasis"; mkdir -p "$shots"
export SHOT_SIZE="${SHOT_SIZE:-1284x816}"
reply="$here/fixtures/emphasis/reply.txt"
start_daemon "$shots"
show="clear; cat $reply"
post() { curl -s -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data "$2" "http://$daemon$1" >/dev/null; }

run() { # name, extra events (JSON, after the reply is shown)
  local name="$1" extra="$2" script="$shots/pills-$1-script.json"
  cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "paste", "text": "$show"},
  {"t": 0.6, "type": "key", "name": "Enter"},
  {"t": 1.2, "type": "mark", "name": "hold:300"}$extra,
  {"t": 3.0, "type": "mark", "name": "end"}
]}
JSON
  run_app "$shots/app-pills-$name.log" --sandbox web-app --script "$script" --snap "$shots/pills-$name.png" --tree "$shots/pills-$name.json" --at mark:end+0.3
  echo "$shots/pills-$name.png"
}

post /api/standin/emphasis '{}'
run 1-terminal ""
# Rows 3 and 4 of the reply's list (Irreversible, Needs you), dragged over.
run 2-selection ',
  {"t": 1.6, "type": "drag", "x": 560, "y": 286, "dx": 260, "dy": 18, "dur": 0.3}'
# The daemon's switch goes off while the app runs: its `emphasis` event.
log="$shots/app-pills-3-off.log"
script="$shots/pills-3-off-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "paste", "text": "$show"},
  {"t": 0.6, "type": "key", "name": "Enter"},
  {"t": 1.0, "type": "mark", "name": "hold:2500"},
  {"t": 2.0, "type": "mark", "name": "end"}
]}
JSON
run_app "$log" --sandbox web-app --script "$script" --snap "$shots/pills-3-off.png" --tree "$shots/pills-3-off.json" --at mark:end+0.6 &
app=$!
for _ in $(seq 300); do grep -q "hold 2500 ms" "$log" 2>/dev/null && break; sleep 0.05; done
sleep 0.3
post /api/standin/emphasis '{"variant": "off"}'
wait $app
echo "$shots/pills-3-off.png"
post /api/standin/emphasis '{}'
# The app has no terminal theme setting yet: the light theme is shown with
# the terminal alone (term-demo), the same TerminalState and drawing.
script="$shots/pills-4-light-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "paste", "text": "$show"},
  {"t": 0.6, "type": "key", "name": "Enter"},
  {"t": 1.0, "type": "mark", "name": "theme:#fdf6e3:auto"},
  {"t": 1.3, "type": "mark", "name": "end"}
]}
JSON
env -u DISPLAY -u WAYLAND_DISPLAY "$bin/term-demo" --daemon "$daemon" --token "$token" --sandbox web-app --pills --no-blink \
  --size "$SHOT_SIZE" --scale "${SHOT_SCALE:-1}" --script "$script" --snap "$shots/pills-4-light.png" --at mark:end+0.5 \
  >"$shots/app-pills-4-light.log" 2>&1
echo "$shots/pills-4-light.png"
# Another window takes a size larger than the pane and prints the reply.
log="$shots/app-pills-5-scaled.log"
script="$shots/pills-5-scaled-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "mark", "name": "hold:2500"},
  {"t": 1.0, "type": "mark", "name": "end"}
]}
JSON
run_app "$log" --sandbox web-app --script "$script" --snap "$shots/pills-5-scaled.png" --tree "$shots/pills-5-scaled.json" --at mark:end+1.0 &
app=$!
for _ in $(seq 300); do grep -q "hold 2500 ms" "$log" 2>/dev/null && break; sleep 0.05; done
sleep 0.5
post /api/standin/other-client "{\"cols\":170,\"rows\":50,\"type\":\"$show\\n\"}"
wait $app
echo "$shots/pills-5-scaled.png"
