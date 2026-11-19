#!/usr/bin/env bash
# The Monitor (an overlay over the pane area, the rail's activity zone),
# headless, against the stand-in daemon with fixtures/monitor/<fixture>.json:
#
#   desktop/scripts/demo-monitor.sh
#   MARKS="ask send" SIZES=720x480 THEME=graphite desktop/scripts/demo-monitor.sh
#
# Marks:
#   ask      the rail's Monitor: shop-web's checkout-copy wants a permission
#            (selected first: the longest waiting), its card, its terminal button;
#   choice   N (the next one that needs you): shop-web main's question, its options;
#   reply    billing-api main: its last reply with keyword pills, the composer;
#   send     words typed for billing-api and Send pressed: the core's dialog
#            with the words in full and where they go (nothing is sent);
#   live     the stand-in's `session_events` arrive: docs-site's "Now" line
#            moves, tax-rules finishes, retry-queue asks a permission;
#   empty    fixtures/monitor/empty.json: no session yet;
#   idle     fixtures/monitor/idle.json: nothing needs you;
#   busy     fixtures/monitor/busy.json: 64 sessions, 9 need you, paged.
# Out: $SBXW_SHOTS/monitor/monitor-<size>-<theme>-<mark>.png, its element tree and log.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/monitor"; mkdir -p "$shots"
build
script() {
  MARK="$1" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="mark", name="wait:rail-monitor")
if m in ("empty",):
    step("rail-monitor", "monitor-empty")
elif m == "idle":
    step("rail-monitor", "monitor-calm")
elif m == "busy":
    step("rail-monitor", "monitor-more")
else:
    step("rail-monitor", "monitor-row-shop-web::checkout-copy")
if m == "choice":
    add(type="type", every=0.02, text="n"); add(type="mark", name="wait:monitor-opt-1-1")
if m in ("reply", "send"):
    step("monitor-row-billing-api:s-bill-main", "monitor-reply")
if m == "send":
    add(type="tap", key="monitor-compose")
    text = "Also add a test with an empty invoice, then stop."
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
    step("monitor-send", "cf-confirm")
if m == "live":
    add(type="tap", key="monitor-row-docs-site:s-docs-main")
    add(type="mark", name="hold:6500")
add(type="mark", name="hold:400")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
theme="${THEME:-dark}"
for size in ${SIZES:-1284x816 720x480}; do
  for m in ${MARKS:-ask choice reply send live empty idle busy}; do
    case "$m" in empty|idle|busy) fx="$m" ;; *) fx=basic ;; esac
    out="$shots/monitor-$size-$theme-$m"
    script "$m" >"$out-script.json"
    start_daemon "$out-standin" --fixture "$here/fixtures/monitor/$fx.json"
    SHOT_SIZE="$size" run_app "$out.log" --script "$out-script.json" \
      --snap "$out.png" --tree "$out.json" --at "mark:$m+0.2"
    cleanup; pids=()
    grep -hE "gave up|no element|script:|monitor:" "$out.log" | sed 's/^\[[^]]*\] //' || true
    echo "$out.png"
  done
done
