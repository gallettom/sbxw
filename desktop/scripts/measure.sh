#!/usr/bin/env bash
# What the desktop app costs, in one run: CPU when idle and under terminal
# output, and what its memory holds (the CPU and memory audits of
# 2026-11-25). A RELEASE build, with the counters on
# (SBXW_DESKTOP_PERF=counts, src/perf.rs: one line a second) and the memory
# report after each scenario (src/shell/memory.rs, SBXW_DESKTOP_MEM=1).
#
# On the Mac (the default there): the real app in its window, against the
# person's real daemon. Quit sbxw first; the script starts it and tells you
# what to do at each step (about 4 minutes):
#   1. idle: the usual panes, a shell prompt focused, untouched;
#   2. idle with something over the panes (Sessions, Commits, any overlay);
#   3. heavy output in a pane on the page shown;
#   4. the same output in a pane on another page, page 1 shown.
# A burst of session events needs the daemon's agents: not scripted there
# (run it with a working team to see one; the "event.session" counter says).
#
#   desktop/scripts/measure.sh
#
# Headless (Linux, or MEASURE_HEADLESS=1): the same scenarios scripted against
# the stand-in daemon (scripts/perf-fixture.py, 12 running sandboxes), plus a
# burst of 100 session events over 10 s. Numbers from a software renderer
# with a virtual frame clock and no cursor blink only show that the probes
# work, never what the Mac pays.
#
# Out: $SBXW_SHOTS/measure/measure.txt (paste it back), app.log, and the
# script's own notes. MEASURE_SECS (30): each scenario's length.
# MEASURE_BIN: another sbxw binary to measure (the installed app's, say:
# /Applications/sbxw.app/Contents/MacOS/sbxw). MEASURE_COUNT=0: without the
# counting allocator (CPU without its small cost; the heap's lines then read
# estimates). MEM_GC=1 (set): each report also runs a full Luau collection
# and says what it freed.
set -euo pipefail
export SBXW_PROFILE=release
source "$(dirname "$0")/lib.sh"
out="$shots_root/measure"; mkdir -p "$out"
secs=${MEASURE_SECS:-30}
headless=${MEASURE_HEADLESS:-}
if [ -z "$headless" ]; then
  if [ "$(uname)" = Darwin ]; then headless=0; else headless=1; fi
fi
build
app="${MEASURE_BIN:-$bin/sbxw}"
log="$out/app.log"; side="$out/side.txt"; meta="$out/meta.txt"; marks="$out/marks"
: >"$side"; rm -f "$marks"
export SBXW_DESKTOP_PERF=counts SBXW_DESKTOP_MEM="${MEASURE_COUNT:-1}" MEM_GC=1

{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "mode: $([ "$headless" = 1 ] && echo "headless, stand-in daemon" || echo "the real app, its window, the real daemon")"
  echo "app: $app"
  echo "checkout: $(git -C "$here" rev-parse --short HEAD 2>/dev/null || echo unknown)$(git -C "$here" diff --quiet 2>/dev/null || echo ' (with changes)')"
  echo "system: $(uname -srm)"
  echo "scenario length: $secs s"
} >"$meta"

# The heavy output: a build log's worth of long lines as fast as the PTY takes them.
heavy="timeout $secs yes 'Compiling sbxw-desktop v0.1.0: warning: unused variable in src/feat/monitor/conv.rs:123:17, help: prefix it with an underscore'"

if [ "$headless" = 1 ]; then
  size=${SHOT_SIZE:-1440x900}
  echo "window: $size pt (headless)" >>"$meta"
  python3 "$here/scripts/perf-fixture.py" >"$out/big.json"
  # Twelve running sandboxes, and the burst: 100 session events, one every
  # 0.1 s, from 4 s after the app connects (the stand-in's session_events).
  python3 - "$out/big.json" >"$out/fixture.json" <<'PY'
import copy, json, sys
d = json.load(open(sys.argv[1]))
sbs = d["daemon"]["sandboxes"]
while len(sbs) < 12:
    x = copy.deepcopy(sbs[0]); x["name"] = f"extra-{len(sbs)}"; sbs.append(x)
for s in sbs:
    s["status"] = "running"
d["app"] = {}
sessions = d["daemon"].get("sessions", [])
ev = []
for i in range(100):
    s = copy.deepcopy(sessions[i % len(sessions)])
    s["state"] = ["working", "idle"][i % 2]
    s["ts"] = s.get("ts", 0) + i
    ev.append({"after": 4.0 + i * 0.1, "session": s})
d["daemon"]["session_events"] = ev
print(json.dumps(d))
PY
  python3 - "$secs" "$heavy" >"$out/script.json" <<'PY'
import json, sys
secs, heavy = int(sys.argv[1]), sys.argv[2]
ev = []; t = 0.5
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.1
def mark(n): add(type="mark", name=n)
def scenario(name, length):
    global t
    mark("perf:begin " + name); t += length
    mark("perf:end " + name); mark("mem:" + name); t += 1.0
mark("wait:row-web-app")
# The burst first: the stand-in times it from the app's connection.
scenario("session events burst (100 in 10 s)", 12)
add(type="tap", key="panel-close-relay"); t += 0.3
add(type="tap", key="pager-per"); mark("wait:pager-per-3:3000"); add(type="tap", key="pager-per-3"); t += 0.3
# Four terminals: three on page 1, the fourth on page 2.
for n in ["chat-research", "chat-release-notes", "sbxw-voice", "better-checkout"]:
    add(type="tap", key="add-" + n); mark("hold:1500"); t += 1.5
add(type="tap", key="page-1"); t += 1.0
scenario("idle", secs)
add(type="tap", key="rail-commits"); t += 1.5
scenario("idle, an overlay over the panes", secs)
add(type="key", name="Escape"); t += 0.5
add(type="paste", text=heavy); add(type="key", name="Enter"); t += 0.5
scenario("heavy output, visible", secs)
t += 2.0
# The fourth pane, on page 2: the command waits 3 s, page 1 shows meanwhile.
add(type="tap", key="page-2"); t += 1.0
add(type="paste", text="sleep 3; " + heavy); add(type="key", name="Enter"); t += 0.3
add(type="tap", key="page-1"); t += 2.7
scenario("heavy output, hidden page", secs)
mark("end")
print(json.dumps({"events": ev}))
PY
  start_daemon "$out/standin" --fixture "$out/fixture.json"
  app_dir=$(dirname "$app"); bin=$app_dir
  SHOT_SIZE="$size" run_app "$log" --script "$out/script.json" --realtime --at mark:end+0.2 --snap "$out/end.png" || true
  cleanup; pids=()
  echo "== session events burst (100 in 10 s)" >>"$side"
  echo "the stand-in sent 100 session events from 4 s after the app connected" >>"$side"
else
  # The real app. Its window, its daemon, the person's own state.
  if pgrep -x sbxw >/dev/null 2>&1; then
    echo "sbxw is running: quit it first (the script starts its own, with the counters on)." >&2
    exit 1
  fi
  echo "Starting $app with the counters on (log: $log)."
  SBXW_DESKTOP_PERF_MARKS="$marks" "$app" >"$log" 2>&1 &
  pid=$!
  pids+=("$pid")
  ask() { printf '\n%s\nPress Enter here when ready. ' "$1"; read -r _; }
  mark() { printf '%s\n' "$@" >>"$marks"; }
  footprint_of() { # the system's own view of the memory, as Activity Monitor's "Memory" (unverified format)
    { footprint "$pid" 2>/dev/null | grep -iE "footprint|total" | head -5 || true
      vmmap --summary "$pid" 2>/dev/null | grep -iE "physical footprint" || true; } | sed 's/^/footprint: /'
  }
  scenario() { # name, what to do first (shown), whether to click into the window
    local name="$1" todo="$2"
    ask "Next: $name. $todo"
    printf 'Click into the app window now; measuring starts in 5 s and lasts %s s. Do not touch anything.\n' "$secs"
    sleep 5
    mark "perf:begin $name"
    sleep "$secs"
    mark "perf:end $name" "mem:$name"
    sleep 3
    { echo "== $name"; footprint_of; } >>"$side"
    echo "Done: $name."
  }
  ask "When the app shows your usual panes (wait for the list to load), make the window the size you use."
  win=$(osascript -e "tell application \"System Events\" to tell (first process whose unix id is $pid) to get size of window 1" 2>/dev/null || echo "unknown (System Events needs Accessibility access; the layout line has the pane area)")
  echo "window: $win" >>"$meta"
  scenario "idle" "Leave a shell prompt focused in a pane, the usual panes shown."
  scenario "idle, an overlay over the panes" "Open Sessions, Commits or Files over the panes and leave it open."
  long="timeout 600 yes 'Compiling sbxw-desktop v0.1.0: warning: unused variable in src/feat/monitor/conv.rs:123:17, help: prefix it with an underscore'"
  scenario "heavy output, visible" "Close the overlay. In a pane on the page shown, paste and run:
    $long"
  ask "Stop it now: Ctrl+C in that pane."
  scenario "heavy output, hidden page" "Make sure you have a second page (a new page from the pager, one pane on it). In a pane on page 2, paste and run:
    $long
then show page 1 (the output goes on, hidden)."
  ask "Stop it now: show page 2 and press Ctrl+C in that pane."
  echo "== session events burst" >>"$side"
  echo "not scripted on the real daemon: it needs agents at work (compare event.session in a run with a working team)" >>"$side"
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  pids=()
  echo "The measured app has quit; start sbxw again as usual."
fi

python3 "$here/scripts/measure-summary.py" "$log" --meta "$meta" --side "$side" --out "$out/measure.txt" >/dev/null
python3 "$here/scripts/measure-summary.py" --parse "$out/measure.txt" >"$out/measure.json" || { echo "the report lacks a scenario's numbers: see $out/measure.txt and $log" >&2; exit 1; }
cat "$out/measure.txt"
echo "Report: $out/measure.txt (paste it back)."
