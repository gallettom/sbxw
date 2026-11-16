#!/usr/bin/env bash
# A whole creation, headless, against the stand-in daemon (no sbx):
#
#   desktop/scripts/demo-create.sh
#
# The top bar's New, the workspace form (a folder browsed to, a name typed,
# an extra folder added), the core's confirmation of what the extra folder
# grants, the daemon's host-access question (its lines, Allow), the bring-up's
# steps arriving on the stream, a failure at the kits step with its reason,
# Retry, and at the end the new sandbox's agent in a spare pane. The stand-in
# fixture is fixtures/create/standin.json (create_consent, create_fail).
#
# The stand-in's state changes during a run, so each picture is its own run
# against a fresh stand-in, up to that mark. A headless run waits for the
# app's own pending calls before drawing on, so the steps in between are not
# pictured here (fixtures/create-jobs/basic.json shows a bring-up mid-way). Out: $SBXW_SHOTS/create/window-<mark>.png,
# the element tree and the app log beside each.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/create"; mkdir -p "$shots"
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
def typ(key, text):
    global t
    add(type="tap", key=key)
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
add(type="tap", key="extra-topbar-create")
mark("chooser", 300)
add(type="tap", key="kind-workspace")
mark("form", 500)
add(type="tap", key="fs-dev"); mark("_dev", 300)
add(type="tap", key="fs-shop"); mark("_shop", 300)
typ("create-name", "shop")
add(type="tap", key="add-backend"); mark("_b", 200)
add(type="tap", key="add-frontend"); mark("_f", 200)
mark("filled", 200)
add(type="tap", key="create-submit")
mark("confirm-grants", 300)
add(type="tap", key="cf-confirm")
mark("consent", 600)
add(type="tap", key="cf-confirm")
mark("failed", 600)
add(type="tap", key="job-retry-shop")
mark("consent-again", 600)
add(type="tap", key="cf-confirm")
mark("ready", 600)
mark("end", 300)
print(json.dumps({"events": ev}, indent=1))
PY
marks=(${MARKS:-chooser form filled confirm-grants consent failed consent-again ready end})
build
for m in "${marks[@]}"; do
  start_daemon "$shots/standin-$m" --fixture "$here/fixtures/create/standin.json"
  SHOT_SIZE="${SHOT_SIZE:-1280x820}" run_app "$shots/window-$m.log" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/window-$m.png"
done
grep -hE "create|confirmed|intent|OpenTerminal" "$shots/window-end.log" | sed 's/^\[[^]]*\] //' | head -40 || true
