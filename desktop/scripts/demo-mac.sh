#!/usr/bin/env bash
# The virtual Mac panel, headless, against the stand-in daemon (no
# terminal-mac, no VM): the stand-in's Mac starts as "supported, not
# installed" (fixtures/mac/not-installed.json) and moves as the daemon would.
#
#   1-panel     Mac in the top bar: the panel, Install terminal-mac;
#   2-install   Install: the CORE's dialog (the release, SHA-256, self-signed);
#   3-missing   confirmed: installed, no VM yet, "Create and start";
#   4-create    Create and start: the core's dialog (macOS downloaded, several GB);
#   5-running   confirmed: running, guest not ready, the one-time setup steps;
#   6-paused    Pause (direct): paused, Resume;
#   7-stop      Resume, then Shut down: the core's dialog, in red;
#   8-stopped   confirmed: stopped, Start.
# Output: $SBXW_SHOTS/mac/<mark>.png, the element tree and logs beside each.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/mac"; mkdir -p "$shots"
python3 - >"$shots/script.json" <<'PY'
import json
ev = []; t = 0.3
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
add(type="tap", key="extra-topbar-mac")
mark("1-panel", 600)
add(type="tap", key="mac-install")
mark("2-install", 300)
add(type="tap", key="cf-confirm")
mark("3-missing", 900)
add(type="tap", key="mac-up")
mark("4-create", 300)
add(type="tap", key="cf-confirm")
mark("5-running", 900)
add(type="tap", key="mac-pause")
mark("6-paused", 900)
add(type="tap", key="mac-up")
add(type="mark", name="hold:900")
add(type="tap", key="mac-stop")
mark("7-stop", 300)
add(type="tap", key="cf-confirm")
mark("8-stopped", 900)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
marks=$(python3 -c 'import json,sys; print(" ".join(e["name"] for e in json.load(open(sys.argv[1]))["events"] if e["type"]=="mark" and e["name"][0].isdigit()))' "$shots/script.json")
for m in $marks; do
  start_daemon "$shots" --fixture "$here/fixtures/mac/not-installed.json"
  run_app "$shots/app-$m.log" --script "$shots/script.json" --snap "$shots/$m.png" --at "mark:$m+0.2" --tree "$shots/$m.json"
  cleanup; pids=()
  echo "$shots/$m.png"
done
echo "--- what the app did (last run)"
grep -hE "intent from (mac|topbar)|mac: (Post|Get)|confirmed|Started|VM |Could not" "$shots/app-8-stopped.log" | sed 's/^\[[^]]*\] //' | cut -c1-160 | head -30
