#!/usr/bin/env bash
# Proof B, headless: click "select" on docs twice, then the screen file is
# edited, broken, and fixed while the app runs. State must survive; the
# broken file must keep the last good view with the error shown.
#
# A headless run has a virtual clock, so the edits cannot be made from this
# shell mid-run. Instead the three versions are written first, and the
# script's marks `screen:<file>` make the app load each one at its time,
# through the same reload code a real edit goes through (the file watcher
# itself is covered by `cargo test screen`). Output: shots/reload-{1..4}-*.png
# and shots/reload-strip.png (Rune's own --strip, panel crops side by side).
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots"
mkdir -p "$shots"
tmp=$(mktemp -d)
v0="$tmp/sandboxes.luau"; v1="$tmp/edited.luau"; v2="$tmp/broken.luau"; v3="$tmp/fixed.luau"
cp "$here/screens/sandboxes.luau" "$v0"
sed 's/text{ "Sandboxes", bold = true }/text{ "Sandboxes (edited live)", bold = true, color = "#f9e2af" }, text{ "a line added while running" }/' "$v0" >"$v1"
cp "$v1" "$v2"; printf '\nreturn {{{\n' >>"$v2"
cp "$v1" "$v3"
cat >"$tmp/script.json" <<JSON
{"events": [
  {"t": 0.3, "type": "tap", "key": "btn-select-docs"},
  {"t": 0.6, "type": "tap", "key": "btn-select-docs"},
  {"t": 1.0, "type": "mark", "name": "screen:$v1"},
  {"t": 1.5, "type": "mark", "name": "screen:$v2"},
  {"t": 2.0, "type": "mark", "name": "screen:$v3"},
  {"t": 2.5, "type": "mark", "name": "end"}
]}
JSON
start_daemon "$shots"
i=1
for step in "0.9 clicked" "1.4 edited" "1.9 broken" "2.4 fixed"; do
  set -- $step
  run_app "$shots/app-reload-$i.log" --screen "$v0" --script "$tmp/script.json" --snap "$shots/reload-$i-$2.png" --at "$1"
  echo "$shots/reload-$i-$2.png"
  i=$((i + 1))
done
# One run, four frames 0.5 s apart from 0.9 s, the left panel cropped from each.
run_app "$shots/app-reload-strip.log" --screen "$v0" --script "$tmp/script.json" \
  --start 0.9 --frames 91 --every 30 --cols 4 --strip "$shots/reload-strip.png" --crop 0,0,320,260
echo "$shots/reload-strip.png"
grep -hE "screen [^ ]+(: | loaded)" "$shots/app-reload-4.log" | sed 's/^\[[^]]*\] //'
rm -rf "$tmp"
