#!/usr/bin/env bash
# Hot reload through the override folder, headless, with real file edits.
# One run, a fixture (no daemon):
#   t=0.3  type "web" in the filter (screen state);
#   t=1.0  a copy of sandboxes.luau appears in the override folder, edited:
#          it replaces the built-in, the filter is kept;
#   t=1.5  the copy is broken: the last good view stays, with the error;
#   t=2.0  the copy is fixed;
#   t=2.5  the copy is deleted: the built-in comes back, the filter still kept.
# At each time the app holds (`hold:` mark) and this script, watching the
# app's log for that hold, edits the file; the app's watcher thread sees it
# as it would in a window. Output: shots/reload-strip.png (five frames 0.5 s
# apart from 0.8 s, the sidebar), the log.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots"; mkdir -p "$shots"
dir=$(mktemp -d); export SBXW_DESKTOP_SCREENS="$dir"
f="$dir/sandboxes.luau"
edited=$(mktemp)
sed 's/text{ "SANDBOXES", size = "sm", weight = "semibold", color = "text" }/text{ "SANDBOXES (edited live)", size = "sm", weight = "semibold", color = "amber" }/' \
  "$here/screens/sandboxes.luau" >"$edited"
grep -q "edited live" "$edited"
script="$shots/reload-script.json"
cat >"$script" <<JSON
{"events": [
  {"t": 0.3, "type": "tap", "key": "filter"},
  {"t": 0.4, "type": "type", "text": "web", "every": 0.05},
  {"t": 1.0, "type": "mark", "name": "hold:1200"},
  {"t": 1.5, "type": "mark", "name": "hold:1201"},
  {"t": 2.0, "type": "mark", "name": "hold:1202"},
  {"t": 2.5, "type": "mark", "name": "hold:1203"},
  {"t": 3.0, "type": "mark", "name": "end"}
]}
JSON
build
log="$shots/app-reload.log"
run_app "$log" --fixture "$here/fixtures/sandboxes/basic.json" --script "$script" \
  --start 0.8 --frames 121 --every 30 --cols 5 --strip "$shots/reload-strip.png" --crop 0,0,250,420 &
app=$!
on_hold() { # wait for the app to enter hold <ms>, then let its watcher settle in
  for _ in $(seq 300); do grep -q "hold $1 ms" "$log" 2>/dev/null && { sleep 0.2; return; }; sleep 0.05; done
  echo "the app never held $1" >&2; exit 1
}
on_hold 1200; cp "$edited" "$f"
on_hold 1201; printf '\nreturn {{{\n' >>"$f"
on_hold 1202; cp "$edited" "$f"; touch -d '+1 second' "$f"
on_hold 1203; rm "$f"
wait $app
rm -rf "$dir" "$edited"
echo "$shots/reload-strip.png"
grep -hE "screen sandboxes" "$log" | sed 's/^\[[^]]*\] //'
