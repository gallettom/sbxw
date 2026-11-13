#!/usr/bin/env bash
# Proof 3, end to end: change state from the UI, edit the Luau file while the
# app runs, break it, fix it. One screenshot per step in desktop/shots/reload-*.png.
# Works on a copy of screens/sandboxes.luau; the repository file is not touched.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots"
screen=$(mktemp --suffix=.luau)
cp "$here/screens/sandboxes.luau" "$screen"
start_app "$shots" --screen "$screen"

# 1. Click "select" on the third sandbox (docs) and on it again: clicks = 2.
eval "$(xdotool getwindowgeometry --shell "$win")"
xdotool mousemove --window "$win" 165 144 click 1 2>/dev/null
sleep 0.3
xdotool mousemove --window "$win" 165 144 click 1 2>/dev/null
sleep 1
snap "$shots/reload-1-clicked.png"

# 2. Edit the file: new title and a new line. State must survive.
sed -i 's/text{ "Sandboxes", bold = true }/text{ "Sandboxes (edited live)", bold = true, color = "#f9e2af" }, text{ "a line added while running" }/' "$screen"
sleep 1.5
snap "$shots/reload-2-edited.png"

# 3. Break it: a syntax error. The last good view must stay, with the error.
printf '\nreturn {{{\n' >> "$screen"
sleep 1.5
snap "$shots/reload-3-broken.png"

# 4. Fix it (drop the broken tail): back to the edited view, state intact.
head -n -2 "$screen" > "$screen.tmp" && mv "$screen.tmp" "$screen"
sleep 1.5
snap "$shots/reload-4-fixed.png"
grep -E "screen .* (loaded|:)" "$shots/app.log" | sed 's/^\[[^]]*\] //'
rm -f "$screen"
