#!/usr/bin/env bash
# Jarvis end to end, headless, against the stand-in daemon and its scripted
# Jarvis. One run: open the panel, start Jarvis, type a message and get an
# answer, ask for the English voice and approve it by typing "yes" (low
# risk), ask for a review command, type "yes" and be told to click (high
# risk), click Approve, ask for a screen change, click Approve and see the
# sandbox list's title change (the stand-in writes the override, the app's
# watcher reloads it). Output: shots/jarvis/demo-*.png (one per step) and
# the logs.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$here/shots/jarvis"; mkdir -p "$shots"
ovr=$(mktemp -d); export SBXW_DESKTOP_SCREENS="$ovr"
script="$shots/demo-script.json"
python3 - >"$script" <<'PY'
import json
ev=[]; t=0.3
def add(**e):
    global t
    e["t"]=round(t,2); ev.append(e); t+=0.2
def say(text, name):
    global t
    add(type="tap", key="jarvis-input")
    add(type="type", every=0.02, text=text); t+=0.02*len(text)
    add(type="key", name="Enter")
    add(type="mark", name="hold:1400")
    add(type="mark", name=name)
add(type="tap", key="jarvis-toggle")
add(type="tap", key="start")
add(type="mark", name="hold:2500")
add(type="mark", name="running")
say("hello Jarvis", "answered")
say("use the english voice", "low-proposed")
say("yes", "low-approved")
add(type="tap", key="jv-next")
say("add a review command", "high-proposed")
say("yes", "high-told")
add(type="tap", key="jv-approve")
add(type="mark", name="hold:900")
add(type="mark", name="high-approved")
add(type="tap", key="jv-next")
say("change the sandboxes screen", "screen-proposed")
add(type="tap", key="jv-approve")
add(type="mark", name="hold:1500")
add(type="mark", name="screen-applied")
add(type="tap", key="jv-next")
add(type="mark", name="end")
print(json.dumps({"events": ev}, indent=1))
PY
marks=(running answered low-proposed low-approved high-proposed high-told high-approved screen-proposed screen-applied end)
for m in "${marks[@]}"; do
  cleanup; pids=(); rm -f "$ovr"/*.luau
  start_daemon "$shots" --jarvis absent --screens-dir "$ovr"
  run_app "$shots/app-$m.log" --script "$script" --snap "$shots/demo-$m.png" --at "mark:$m+0.05"
  echo "$shots/demo-$m.png"
done
grep -hE "intent from jarvis|jarvis|screen sandboxes" "$shots/app-end.log" | sed 's/^\[[^]]*\] //' | grep -v "loaded from built-in" || true
grep -h "standin:" "$shots/standin.log" | tail -20 || true
