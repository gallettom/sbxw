#!/usr/bin/env bash
# The Monitor (a base view of the pane area, instead of the terminals; the
# rail's activity zone, the pager's view switch),
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
#   busy     fixtures/monitor/busy.json: 64 sessions, 9 need you, paged;
#   switch   the pager's view switch (Tabs | Grid | Monitor) over the terminals;
#   base     the Monitor as the base: its header's view switch, no cross;
#   under    the Monitor's Files link: the explorer over the Monitor;
#   back     Escape there: back to the Monitor, the same session;
#   attach   two files dropped on billing-api's session: their chips under the box;
#   attach-dialog  a third, named like a secret (amber), words typed, Send:
#            the core's one dialog (words, each file, the total, its box);
#   fold     fixtures/monitor/folding.json: one row per place (studio's main
#            checkout reported by three sessions shows once, its last prompt
#            another session's message shown as such; lab's two live terminals).
# Out: $SBXW_SHOTS/monitor/monitor-<size>-<theme>-<mark>.png, its element tree and log.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/monitor"; mkdir -p "$shots"
build
# Invented files to attach (never this machine's own).
drop=/tmp/sbxw-demo/attach; mkdir -p "$drop"
printf '# Brief\n\nThe invoice export, as agreed.\n' >"$drop/brief.md"
printf 'name,qty\nmug,2\n' >"$drop/order list.csv"
printf 'EXAMPLE=1\n' >"$drop/.env.local"
script() {
  DROP="$drop" MARK="$1" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="mark", name="wait:rail-monitor")
if m == "switch":
    add(type="mark", name="wait:pager-view-monitor")
elif m == "fold":
    step("rail-monitor", "monitor-row-studio:s-studio-1")
    if not os.environ.get("SIZE", "").startswith("720"):
        step("monitor-row-studio:s-studio-1", "monitor-from")
elif m in ("empty",):
    step("rail-monitor", "monitor-empty")
elif m == "idle":
    step("rail-monitor", "monitor-calm")
elif m == "busy":
    step("rail-monitor", "monitor-more")
else:
    step("rail-monitor", "monitor-row-shop-web::checkout-copy")
if m == "choice":
    # N: the next session that needs you; then its row (a narrow view opens it).
    add(type="type", every=0.02, text="n"); add(type="mark", name="hold:300")
    step("monitor-row-shop-web:s-shop-main", "monitor-opt-1-1")
if m in ("reply", "send", "attach", "attach-dialog"):
    # A short window: the idle rows are below the fold of the list.
    add(type="wheel", key="monitor-row-shop-web:s-shop-main", dy=-600); add(type="mark", name="hold:300")
    step("monitor-row-billing-api:s-bill-main", "monitor-reply")
if m == "send":
    add(type="tap", key="monitor-compose")
    text = "Also add a test with an empty invoice, then stop."
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
    step("monitor-send", "cf-confirm")
if m in ("attach", "attach-dialog"):
    D = os.environ["DROP"]
    add(drop_files={"paths": [f"{D}/brief.md", f"{D}/order list.csv"], "key": "monitor-detail", "hover": 0.2})
    add(type="mark", name="wait:monitor-attached")
if m == "attach-dialog":
    add(drop_files={"paths": [f"{D}/.env.local"], "key": "monitor-detail", "hover": 0.2}); add(type="mark", name="hold:300")
    add(type="tap", key="monitor-compose")
    text = "Use these for the export test."
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
    step("monitor-send", "cf-confirm")
if m in ("under", "back"):
    # A narrow window lists the sessions: a row opens one, with its links.
    if os.environ.get("SIZE", "").startswith("720"):
        step("monitor-row-shop-web::checkout-copy", "monitor-back")
    step("monitor-files", "overlay-files")
if m == "back":
    add(type="key", name="Escape"); add(type="mark", name="wait-gone:overlay-files")
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
  for m in ${MARKS:-ask choice reply send live empty idle busy switch base under back attach attach-dialog fold}; do
    case "$m" in empty|idle|busy) fx="$m" ;; fold) fx=folding ;; *) fx=basic ;; esac
    out="$shots/monitor-$size-$theme-$m"
    SIZE="$size" script "$m" >"$out-script.json"
    start_daemon "$out-standin" --fixture "$here/fixtures/monitor/$fx.json"
    SHOT_SIZE="$size" run_app "$out.log" --script "$out-script.json" \
      --snap "$out.png" --tree "$out.json" --at "mark:$m+0.2"
    cleanup; pids=()
    grep -hE "gave up|no element|script:|monitor:" "$out.log" | sed 's/^\[[^]]*\] //' || true
    echo "$out.png"
  done
done
