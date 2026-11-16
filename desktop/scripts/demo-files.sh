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
def mark(n):
    add(type="mark", name=n)
# A tap, then a real-time wait until what it brings is drawn (the core's
# `wait:<key>` / `wait-gone:<key>` marks, 10 s at most, logged with the time
# waited), then the mark named for the picture.
def step(key, until, name=None, gone=False):
    if isinstance(key, dict):
        add(**key)
    else:
        add(type="tap", key=key)
    add(type="mark", name=("wait-gone:" if gone else "wait:") + until)
    if name: mark(name)
add(type="tap", key="row-web-app")
add(type="mark", name="wait:pane-1-x-project")
if not os.environ.get("LONG"):
    step("pane-1-x-project", "files-row-report.md", "artifacts")
    step("files-row-report.md", "files-preview-markdown", "preview-markdown")
    step("files-row-wireframe.png", "files-preview-image", "preview-picture")
    step("files-open-with", "cf-confirm", "open-dialog")
    step("cf-cancel", "cf-dialog", gone=True)
    step("files-tab-files", "files-row-/Users/me/web/.sbxw-artifacts/report.md")
    step("files-up", "files-preview-markdown", "browse")
    step("files-row-/Users/me/web/install.sh", "files-preview-text")
    step("files-open-with", "cf-error", "open-refused")
    step("cf-cancel", "cf-dialog", gone=True)
    step("files-tab-recent", "files-show-/tmp/build.log", "recent")
    step({"drop_files": {"paths": [f"{SHOTS}/drop/brief.md"], "key": "files-drop", "hover": 0.2}}, "files-staged-1", "staged")
    step("files-upload-send-1", "cf-confirm", "upload-confirm")
    step("cf-confirm", "files-staged-1", "uploaded", gone=True)
    step({"drop_files": {"paths": [f"{SHOTS}/drop/order list.csv"], "key": "pane-1", "hover": 0.2}}, "cf-confirm", "pane-drop-confirm")
    step("cf-cancel", "cf-dialog", gone=True)
    step("files-codemap", "codemap-docs", "codemap")
    step("codemap-docs", "codemap-docs-net")
    step("codemap-docs-net", "codemap-outline-net-proxy", "codemap-net")
    step("codemap-outline-net-proxy", "codemap-whole", "codemap-section")
    step("codemap-diagram", "cf-ack", "diagram-dialog")
else:
    # LONG=1: the dialogs with long paths, at the smallest window.
    step("pane-1-x-project", "files-row-report.md")
    step("files-tab-recent", "files-show-" + os.environ["LONGREMOTE"])
    step("files-row-" + os.environ["LONGREMOTE"], "files-preview-markdown")
    step("files-open-with", "cf-confirm", "long-open")
    step("cf-cancel", "cf-dialog", gone=True)
    step({"drop_files": {"paths": [os.environ["LONGFILE"]], "key": "files-drop", "hover": 0.2}}, "files-staged-1")
    step("files-upload-send-1", "cf-confirm", "long-upload")
mark("end")
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
