#!/usr/bin/env bash
# The Project panel and the code map, headless, against the stand-in daemon
# (no sbx):
#
#   desktop/scripts/demo-files.sh            # every mark
#   MARKS="artifacts open-dialog" desktop/scripts/demo-files.sh
#
# web-app's agent opened, its pane bar's Project: the deliverables, a
# Markdown preview, a picture read raw and decoded by the core, Open (the
# core downloads the file, sniffs it, and its dialog says what it is), the
# workspace with its README, a script whose Open the core refuses, Recent, a
# file dropped on the panel and uploaded through the core's dialog, a file
# dropped on the terminal pane (the dialog at once), the code map: index,
# a file, a section followed, the diagram's dialog.
#
# Downloads go to $shots/root (RUNE_DIRS_ROOT), never to your Downloads.
# The stand-in's state changes during a run, so each picture is its own run
# against a fresh stand-in, up to that mark. Out:
# $SBXW_SHOTS/files/window-<mark>.png, the element tree and the app log beside each.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/files"; mkdir -p "$shots/drop"
script="$shots/window-script.json"
printf '# Brief\n\nThe checkout copy, as agreed on Friday.\n' >"$shots/drop/brief.md"
printf 'name,qty\nmug,2\n' >"$shots/drop/order list.csv"
# A long local path, for the dialogs at the smallest window (LONG=1).
long="$shots/drop/$(printf 'a-very-long-folder-name-for-the-dialog-test/%.0s' 1 2 3 4)"
mkdir -p "$long"; printf 'x' >"$long/client-brief-signed-final.pdf"
export RUNE_DIRS_ROOT="$shots/root"
LONGREMOTE="/tmp/$(printf 'deeply-nested-folder-of-agent-output/%.0s' 1 2 3 4 5 6 7)quarterly-report-final-v2.md" LONGFILE="$long/client-brief-signed-final.pdf" SHOTS="$shots" python3 - >"$script" <<'PY'
import json, os
SHOTS = os.environ["SHOTS"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
add(type="tap", key="row-web-app"); mark("_pane", 800)
add(type="tap", key="pane-1-x-project"); mark("artifacts", 600)
add(type="tap", key="files-row-report.md"); mark("preview-markdown", 800)
add(type="tap", key="files-row-wireframe.png"); mark("preview-picture", 800)
add(type="tap", key="files-open-with"); mark("open-dialog", 1200)
add(type="tap", key="cf-cancel"); mark("_closed", 200)
add(type="tap", key="files-tab-files"); mark("_files", 1000)
add(type="tap", key="files-up"); mark("browse", 1200)
add(type="tap", key="files-row-/Users/me/web/install.sh"); mark("_script", 800)
add(type="tap", key="files-open-with"); mark("open-refused", 1200)
add(type="tap", key="cf-cancel"); mark("_closed2", 200)
add(type="tap", key="files-tab-recent"); mark("recent", 500)
add(drop_files={"paths": [f"{SHOTS}/drop/brief.md"], "key": "files-drop", "hover": 0.2}); t += 0.3
mark("staged", 400)
add(type="tap", key="files-upload-send-1"); mark("upload-confirm", 400)
add(type="tap", key="cf-confirm"); mark("uploaded", 1200)
add(drop_files={"paths": [f"{SHOTS}/drop/order list.csv"], "key": "pane-1", "hover": 0.2}); t += 0.3
mark("pane-drop-confirm", 500)
add(type="tap", key="cf-cancel"); mark("_closed3", 200)
add(type="tap", key="files-codemap"); mark("codemap", 1200)
add(type="tap", key="codemap-docs"); mark("_menu", 200)
add(type="tap", key="codemap-docs-net"); mark("codemap-net", 400)
add(type="tap", key="codemap-outline-net-proxy"); mark("codemap-section", 600)
add(type="tap", key="codemap-diagram"); mark("diagram-dialog", 1200)
mark("end", 300)
# LONG=1: the dialogs with long paths, at the smallest window.
if os.environ.get("LONG"):
    ev.clear(); t = 0.6
    add(type="tap", key="row-web-app"); mark("_pane", 800)
    add(type="tap", key="pane-1-x-project"); mark("_panel", 600)
    add(type="tap", key="files-tab-recent"); mark("_recent", 500)
    add(type="tap", key="files-row-" + os.environ["LONGREMOTE"]); mark("_sel", 800)
    add(type="tap", key="files-open-with"); mark("long-open", 1200)
    add(type="tap", key="cf-cancel"); mark("_c", 200)
    add(drop_files={"paths": [os.environ["LONGFILE"]], "key": "files-drop", "hover": 0.2}); t += 0.3
    mark("_staged", 400)
    add(type="tap", key="files-upload-send-1"); mark("long-upload", 500)
    mark("end", 300)
print(json.dumps({"events": ev}, indent=1))
PY
marks=(${MARKS:-artifacts preview-markdown preview-picture open-dialog browse open-refused recent staged upload-confirm uploaded pane-drop-confirm codemap codemap-net codemap-section diagram-dialog})
build
for m in "${marks[@]}"; do
  rm -rf "$RUNE_DIRS_ROOT"
  start_daemon "$shots/standin-$m" --fixture "$here/fixtures/files/standin.json"
  SHOT_SIZE="${SHOT_SIZE:-1400x900}" SBXW_DESKTOP_STATE="$shots/state-$m.json" run_app "$shots/window-$m.log" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/window-$m.png"
done
grep -hE "files|codemap|intent|upload|download" "$shots/window-${marks[-1]}.log" | sed 's/^\[[^]]*\] //' | head -60 || true
