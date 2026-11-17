#!/usr/bin/env bash
# The status bar, headless, against the stand-in daemon (no sbx):
#
#   desktop/scripts/demo-statusbar.sh
#
# A chat sandbox created by another client (curl to the stand-in, as the web
# UI would; a bring-up this app posts itself holds a headless run until its
# answer, so its steps could not be pictured); its bring-up in the status
# bar (name, step, progress line) while the stand-in plays its steps (1.5 s
# each, fixtures/create/statusbar.json); a press on it opens the "New
# sandboxes" panel; the CPU popover from the stand-in's stats (partial: every
# other sandbox without a report) and the cost popover; then the bring-up
# ends, "created" shows and clears. Out: $SBXW_SHOTS/statusbar/window-<mark>.png,
# the element tree and the app log beside each.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/statusbar"; mkdir -p "$shots"
script="$shots/window-script.json"
python3 - >"$script" <<'PY'
import json
ev = []; t = 0.4
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
add(type="mark", name="wait:status-jobs-track:8000")
mark("midway", 3500)
add(type="tap", key="status-cpu")
mark("cpu", 300)
add(type="key", name="Escape")
add(type="tap", key="status-cost")
mark("cost", 300)
add(type="tap", key="status-pop-scrim")
add(type="tap", key="status-jobs")
mark("panel", 300)
add(type="mark", name="wait-gone:status-jobs-track:15000")
mark("created", 100)
add(type="mark", name="wait-gone:status-jobs:8000")
mark("end", 300)
print(json.dumps({"events": ev}, indent=1))
PY
marks=(${MARKS:-midway cpu cost panel created end})
build
for m in "${marks[@]}"; do
  start_daemon "$shots/standin-$m" --fixture "$here/fixtures/create/statusbar.json" --stats partial
  # Another client starts the bring-up a second after the app is up.
  (sleep 1.5; curl -s -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' -d '{"name":"notes"}' "http://$daemon/api/sandboxes/chat" >/dev/null) &
  pids+=($!)
  SHOT_SIZE="${SHOT_SIZE:-1284x816}" run_app "$shots/window-$m.log" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/window-$m.png"
done
grep -hE "waited|gave up|no element" "$shots/window-end.log" | sed 's/^\[[^]]*\] //' || true
