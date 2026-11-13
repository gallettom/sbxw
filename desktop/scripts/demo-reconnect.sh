#!/usr/bin/env bash
# Daemon restart, headless: the app runs against the stand-in with a token
# FILE (as it reads ~/.sbxw/run/ui-token). While the app holds (`hold:` mark),
# this script kills the stand-in and starts a new one on the same port with a
# NEW token in that file, as a daemon restart does. The link reconnects with
# the new token, the pane's terminal reconnects on its own, and typing works
# again. Output: shots/reconnect-before.png, shots/reconnect-after.png, the log.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots"; mkdir -p "$shots"
build
port=$(free_port); addr="127.0.0.1:$port"
tokdir=$(mktemp -d); tokfile="$tokdir/ui-token"
script="$shots/reconnect-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "type", "every": 0.02, "text": "echo before the restart"},
  {"t": 0.9, "type": "key", "name": "Enter"},
  {"t": 1.2, "type": "mark", "name": "hold:7000"},
  {"t": 1.5, "type": "type", "every": 0.02, "text": "echo after the restart"},
  {"t": 2.1, "type": "key", "name": "Enter"},
  {"t": 2.5, "type": "mark", "name": "end"}
]}
JSON
start_standin() { "$bin/standin" "$addr" "$1" --token-file "$tokfile" >>"$shots/standin-reconnect.log" 2>&1 & sp=$!; for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done; }
run() { # run <log> <rune args...>
  local log="$1"; shift
  env -u DISPLAY -u WAYLAND_DISPLAY "$bin/sbxw" --daemon "$addr" --token-file "$tokfile" --sandbox web-app \
    --size 1280x800 --scale 1 --script "$script" "$@" >"$log" 2>&1
}
: >"$shots/standin-reconnect.log"
start_standin "first-$RANDOM"
run "$shots/app-reconnect-before.log" --snap "$shots/reconnect-before.png" --at 1.1
kill $sp; wait $sp 2>/dev/null || true
start_standin "first-$RANDOM"
log="$shots/app-reconnect.log"
run "$log" --snap "$shots/reconnect-after.png" --at mark:end+0.5 &
app=$!
for _ in $(seq 300); do grep -q "hold 7000 ms" "$log" 2>/dev/null && break; sleep 0.05; done
sleep 0.3
kill $sp; wait $sp 2>/dev/null || true
echo "stand-in stopped" >>"$shots/standin-reconnect.log"
sleep 1.5
start_standin "second-$RANDOM"
wait $app
kill $sp 2>/dev/null || true
rm -rf "$tokdir"
echo "$shots/reconnect-before.png"
echo "$shots/reconnect-after.png"
grep -hE "daemon:|terminal|reconnect|Lost|No sbxw" "$log" | sed 's/^\[[^]]*\] //' | grep -v "poll blocked" | head -20
