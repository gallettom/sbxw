#!/usr/bin/env bash
# Sessions, the feature `monitor` (a base view of the pane area, instead of
# the terminals: the top bar's GUI | TUI; Lead Mode beside it),
# headless, against the stand-in daemon with fixtures/monitor/<fixture>.json:
#
#   desktop/scripts/demo-monitor.sh
#   MARKS="ask send" SIZES=720x480 THEME=graphite desktop/scripts/demo-monitor.sh
#
# Marks:
#   ask      Sessions (the top bar's GUI): shop-web's checkout-copy wants a permission
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
#   switch   the pager's view switch (Tabs | Grid) over the terminals;
#   base     Sessions as the base (GUI in the top bar): its header, no cross;
#   top-tui  the top bar in TUI, Lead Mode Off (the terminals);
#   top-gui  the top bar in GUI (Sessions), Lead Mode Off;
#   lead-tui Lead Mode On in TUI: only the team leaders' terminals;
#   lead-gui Lead Mode On in GUI: Sessions lists the leaders and their threads;
#   reply    (also) billing-api main's conversation: two days, a folded step group,
#            the lead's message, a compaction, the reply with pills;
#   right    fixtures/monitor/right.json: billing-api main's conversation with the person's
#            messages on the right (short, long, unbroken) and the agents' on the left;
#   steps    its last group of steps unfolded, then one step's output;
#   injected the lead's long message unfolded (a "From lead" card);
#   partial  docs-site main: an older hook, the summary and what to do;
#   conv-live retry-queue's history changes live (a result, a provisional reply replaced);
#   under    the Monitor's Files link: the explorer over the Monitor;
#   back     Escape there: back to the Monitor, the same session;
#   attach   two files dropped on billing-api's session: their chips under the box;
#   attach-dialog  a third, named like a secret (amber), words typed, Send:
#            the core's one dialog (words, each file, the total, its box);
#   attach-sent  its box ticked and Send pressed: the stand-in takes the
#            uploads, then the push of the words and the daemon's paths;
#   hidden-terms  the sidebar hidden (its header's button) over the terminals;
#   hidden   the sidebar hidden with the Monitor as the base (the rail's
#            "Show sandboxes" at the top, its amber badge);
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
add(type="mark", name="wait:top-view-gui")
if m in ("hidden", "hidden-terms"):
    step("sidebar-hide", "rail-sidebar")
if m == "hidden-terms":
    pass
elif m == "switch":
    add(type="mark", name="wait:pager-view-tabs")
elif m == "fold":
    step("top-view-gui", "monitor-row-studio:s-studio-1")
    if not os.environ.get("SIZE", "").startswith("720"):
        step("monitor-row-studio:s-studio-1", "monitor-from")
elif m in ("empty",):
    step("top-view-gui", "monitor-empty")
elif m == "idle":
    step("top-view-gui", "monitor-calm")
elif m == "busy":
    step("top-view-gui", "monitor-more")
elif m in ("top-tui", "lead-tui"):
    pass
elif m != "hidden-terms":
    step("top-view-gui", "monitor-row-shop-web::checkout-copy")
if m in ("lead-tui", "lead-gui"):
    # Lead Mode On from the top bar: the leaders' terminals, or their sessions.
    add(type="mark", name="wait:top-lead"); add(type="mark", name="hold:600")
    add(type="tap", key="top-lead"); add(type="mark", name="hold:800")
if m == "choice":
    # N: the next session that needs you; then its row (a narrow view opens it).
    add(type="type", every=0.02, text="n"); add(type="mark", name="hold:300")
    step("monitor-row-shop-web:s-shop-main", "monitor-opt-1-1")
if m in ("reply", "right", "send", "attach", "attach-dialog", "attach-sent", "steps", "injected"):
    # A short window: the idle rows are below the fold of the list.
    add(type="wheel", key="monitor-row-shop-web:s-shop-main", dy=-600); add(type="mark", name="hold:300")
    step("monitor-row-billing-api:s-bill-main", "monitor-conv-billing-api:s-bill-main")
if m == "steps":
    # Its last group of steps unfolded, then one step's output.
    step("monitor-steps-g24", "monitor-step-s34")
    step("monitor-step-s34", "monitor-out-s34")
if m == "injected":
    # The lead's message, unfolded in full (it is a "From lead" card, never "You").
    add(type="wheel", key="monitor-conv-billing-api:s-bill-main", dy=-300); add(type="mark", name="hold:300")
    step("monitor-fold-22", "monitor-from-22")
if m == "partial":
    # docs-site's hook is older: the summary, and the line that says what to do.
    if os.environ.get("SIZE", "").startswith("720"):
        add(type="wheel", key="monitor-row-shop-web:s-shop-main", dy=-300); add(type="mark", name="hold:300")
    step("monitor-row-docs-site:s-docs-main", "monitor-conv-note")
if m == "conv-live":
    # retry-queue: its test result comes in, then a provisional reply, replaced by the transcript's.
    step("monitor-row-billing-api::retry-queue", "monitor-conv-billing-api::retry-queue")
    add(type="mark", name="hold:8000")
if m == "send":
    add(type="tap", key="monitor-compose")
    text = "Also add a test with an empty invoice, then stop."
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
    step("monitor-send", "cf-confirm")
if m in ("attach", "attach-dialog", "attach-sent"):
    D = os.environ["DROP"]
    add(drop_files={"paths": [f"{D}/brief.md", f"{D}/order list.csv"], "key": "monitor-detail", "hover": 0.2})
    add(type="mark", name="wait:monitor-attached")
if m in ("attach-dialog", "attach-sent"):
    add(drop_files={"paths": [f"{D}/.env.local"], "key": "monitor-detail", "hover": 0.2}); add(type="mark", name="hold:300")
    add(type="tap", key="monitor-compose")
    text = "Use these for the export test."
    add(type="type", every=0.02, text=text); t += 0.02 * len(text)
    step("monitor-send", "cf-confirm")
if m == "attach-sent":
    add(type="tap", key="cf-ack"); add(type="mark", name="hold:200")
    add(type="tap", key="cf-confirm"); add(type="mark", name="wait-gone:cf-dialog")
    add(type="mark", name="hold:1500")
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
  for m in ${MARKS:-ask choice reply send live empty idle busy switch base under back attach attach-dialog attach-sent fold hidden hidden-terms top-tui top-gui lead-tui lead-gui steps injected partial conv-live}; do
    case "$m" in empty|idle|busy) fx="$m" ;; fold) fx=folding ;; right) fx=right ;; *) fx=basic ;; esac
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
