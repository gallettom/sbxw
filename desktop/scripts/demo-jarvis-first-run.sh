#!/usr/bin/env bash
# The first real run on a Mac, replayed against the stand-in (JARVIS.md,
# "First real run: two fixes"): the panel opened while Jarvis is absent,
# "changez la voix" sent and refused (the error beside the input, the text
# kept, "Start Jarvis and send"), that button pressed (Jarvis starts, the
# kept text leaves by itself, as asked), Jarvis's first answer is a question
# with numbered options (AskUserQuestion), "2" answers it, the second answer.
# The stand-in keeps an older Jarvis session left as `working`, and sends one
# `session` event per session on every hook, as the daemon does. A second
# run shows that editing the input hides the error, and a third the notice
# when another client (a browser tab) follows Jarvis too.
# Output: $SBXW_SHOTS/jarvis/first-*.png and the logs.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/jarvis"; mkdir -p "$shots"
script="$shots/first-script.json"
python3 - >"$script" <<'PY'
import json
ev=[]; t=0.3
def add(**e):
    global t
    e["t"]=round(t,2); ev.append(e); t+=0.2
def typed(text):
    global t
    add(type="tap", key="jarvis-input")
    add(type="type", every=0.02, text=text); t+=0.02*len(text)
add(type="tap", key="jarvis-toggle")
add(type="mark", name="hold:600")
typed("changez la voix")
add(type="key", name="Enter")
add(type="mark", name="hold:500")
add(type="mark", name="refused")
add(type="tap", key="start_send")
add(type="mark", name="hold:3000")
add(type="mark", name="hold:2000")
add(type="mark", name="asked")
typed("2")
add(type="key", name="Enter")
add(type="mark", name="hold:2500")
add(type="mark", name="answered")
add(type="mark", name="hold:2000")
add(type="mark", name="end")
print(json.dumps({"events": ev}, indent=1))
PY
edit="$shots/first-edit.json"
python3 - >"$edit" <<'PY'
import json
ev=[{"t":0.3,"type":"tap","key":"jarvis-toggle"},{"t":0.9,"type":"tap","key":"jarvis-input"},
    {"t":1.0,"type":"type","every":0.02,"text":"changez la voix"},{"t":1.5,"type":"key","name":"Enter"},
    {"t":2.0,"type":"key","name":"Backspace"},{"t":2.4,"type":"mark","name":"edited"}]
print(json.dumps({"events": ev}, indent=1))
PY
for m in refused asked answered end; do
  cleanup; pids=()
  start_daemon "$shots" --jarvis absent --stale-session yes
  run_app "$shots/app-first-$m.log" --script "$script" --snap "$shots/first-$m.png" --at "mark:$m+0.05"
  echo "$shots/first-$m.png"
done
cleanup; pids=()
start_daemon "$shots" --jarvis absent
run_app "$shots/app-first-edited.log" --script "$edit" --snap "$shots/first-edited.png" --at "mark:edited+0.05"
echo "$shots/first-edited.png"
cleanup; pids=()
start_daemon "$shots" --jarvis running --clients 2
run_app "$shots/app-first-others.log" --script "$edit" --snap "$shots/first-others.png" --at "mark:edited+0.05"
echo "$shots/first-others.png"
grep -hE "jarvis" "$shots/app-first-end.log" | sed 's/^\[[^]]*\] //' | grep -v "loaded from built-in" || true
grep -h "standin:" "$shots/standin.log" | tail -12 || true
