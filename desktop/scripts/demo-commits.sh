#!/usr/bin/env bash
# The Commits view (an overlay over the pane area), headless, against the
# stand-in daemon and its built-in history (fixtures/commits/standin.json):
#
#   desktop/scripts/demo-commits.sh                 # every mark, both sizes
#   MARKS="list" SIZES=720x480 desktop/scripts/demo-commits.sh
#
# Marks, each its own run: `list` (the rail's Commits: every sandbox, newest
# first; chat-0412 and web-api "not available"), `expanded` (a row opened
# in place, a file's diff under it), `sandbox` (opened from web-app's pane
# menu: filtered to it), `filtered` (the text filter and Only unmerged).
# Out: $SBXW_SHOTS/commits/commits-<size>-<mark>.png and its element tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/commits"; mkdir -p "$shots"
build
row=$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1]))["daemon"]["commits"]["web-app"]; print("web-app:"+d[0]["hash"])' "$here/fixtures/commits/standin.json")
script() { # mark
  MARK="$1" ROW="$row" python3 - <<'PY'
import json, os
m = os.environ["MARK"]; row = os.environ["ROW"]
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="mark", name="wait:row-web-app")
if m == "sandbox":
    step("row-web-app", "pane-1-more")
    step("pane-1-more", "pane-1-x-commits")
    step("pane-1-x-commits", "commits-row-" + row)
else:
    step("rail-commits", "commits-row-" + row)
if m == "expanded":
    step("commits-row-" + row, "commits-file-src/checkout/Payment.tsx")
    step("commits-file-src/checkout/Payment.tsx", "commits-diff-src/checkout/Payment.tsx")
if m == "filtered":
    add(type="tap", key="commits-filter"); add(type="type", text="checkout", every=0.02); t += 0.4
    step("commits-unmerged", "commits-row-" + row)
add(type="mark", name="hold:400")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for size in ${SIZES:-1284x816 720x480}; do
  for m in ${MARKS:-list expanded sandbox filtered}; do
    script "$m" >"$shots/script-$m.json"
    start_daemon "$shots/standin-$size-$m"
    SHOT_SIZE="$size" run_app "$shots/commits-$size-$m.log" --script "$shots/script-$m.json" \
      --snap "$shots/commits-$size-$m.png" --tree "$shots/commits-$size-$m.json" --at "mark:$m+0.2"
    cleanup; pids=()
    grep -hE "gave up|no element|script:" "$shots/commits-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
    echo "$shots/commits-$size-$m.png"
  done
done
