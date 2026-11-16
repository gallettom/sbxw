#!/usr/bin/env bash
# The relay, headless, against the stand-in daemon (no sbx):
#
#   desktop/scripts/demo-relay.sh            # every mark
#   MARKS="panel route-confirm" desktop/scripts/demo-relay.sh
#
# Each kind of request, from the Requests panel: a question routed to api
# (the core's dialog shows the text and where it goes), api's scripted
# answer reviewed and released; a screenshot request answered in words; a
# round of questions answered (a recommendation used, one typed), masked,
# confirmed; an agent's multiple-choice question (shop) picked. Then the
# Jarvis panel follows web-app: target picked, Follow on, words typed go as a
# draft in the core's dialog, web-app's scripted reply shows in the log; a
# /grill-me round from web-app lands on the Requests panel's form.
#
# The stand-in fixture is fixtures/relay/standin.json. The stand-in's state
# changes during a run, so each picture is its own run against a fresh
# stand-in, up to that mark. Out: shots/relay/window-<mark>.png, the element
# tree and the app log beside each.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots/relay"; mkdir -p "$shots"
script="$shots/window-script.json"
python3 - >"$script" <<'PY'
import json
ev = []; t = 0.6
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
mark("panel", 300)
# A question: routed to api, answered by its scripted agent, released.
add(type="tap", key="relay-to-r-1-api")
mark("route-confirm", 300)
add(type="tap", key="cf-confirm")
mark("answered", 1500)
add(type="tap", key="relay-send-r-1")
mark("send-confirm", 300)
add(type="tap", key="cf-confirm")
mark("sent", 600)
# A screenshot request, answered in words.
add(type="tap", key="relay-req-r-2"); mark("_shot", 200)
typ("relay-caption-r-2", "The save button sits at the right; the title wraps on two lines at 375px.")
mark("shot-typed", 200)
add(type="tap", key="relay-send-r-2")
mark("shot-confirm", 300)
add(type="tap", key="cf-confirm")
mark("_shot-sent", 600)
# A round: a recommendation used, one answer typed, masked, confirmed.
add(type="tap", key="relay-req-r-3"); mark("_round", 200)
add(type="tap", key="relay-rec-r-3-1"); mark("_rec", 200)
typ("relay-ans-r-3-2", "Keep the cookie, ask Jeanne Martin about the mobile app")
mark("round-filled", 200)
add(type="tap", key="relay-send-r-3")
mark("round-confirm", 900)
add(type="tap", key="cf-confirm")
mark("_round-sent", 600)
# An agent's choice.
add(type="tap", key="agent-shop-1-2")
mark("picked", 800)
# Following web-app from the Jarvis panel.
add(type="tap", key="jarvis-toggle"); mark("_jarvis", 300)
add(type="tap", key="jarvis-target"); mark("_menu", 200)
add(type="tap", key="jarvis-target-web-app"); mark("_picked", 200)
add(type="tap", key="jarvis-follow"); mark("_follow", 200)
typ("jarvis-input", "run the checkout tests")
add(type="key", name="Enter")
mark("follow-draft", 900)
add(type="tap", key="cf-confirm")
mark("follow-reply", 1500)
typ("jarvis-input", "grill me on the migration")
add(type="key", name="Enter")
mark("_grill-draft", 900)
add(type="tap", key="cf-confirm")
mark("follow-round", 1500)
mark("end", 300)
print(json.dumps({"events": ev}, indent=1))
PY
marks=(${MARKS:-panel route-confirm answered send-confirm sent shot-typed shot-confirm round-filled round-confirm picked follow-draft follow-reply follow-round})
build
for m in "${marks[@]}"; do
  start_daemon "$shots/standin-$m" --fixture "$here/fixtures/relay/standin.json"
  SHOT_SIZE="${SHOT_SIZE:-1400x860}" SBXW_DESKTOP_PREFS="$shots/prefs-$m.json" run_app "$shots/window-$m.log" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  cleanup; pids=()
  echo "$shots/window-$m.png"
done
grep -hE "relay|follow|intent|standin" "$shots/window-${marks[-1]}.log" | sed 's/^\[[^]]*\] //' | head -60 || true
