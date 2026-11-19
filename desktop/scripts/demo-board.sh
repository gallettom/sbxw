#!/usr/bin/env bash
# The Board (an overlay over the pane area), headless, on the fixture
# fixtures/board/basic.json (three invented teams; no daemon):
#
#   desktop/scripts/demo-board.sh                    # every mark, both sizes, both themes
#   MARKS="running" SIZES=720x480 THEMES=graphite desktop/scripts/demo-board.sh
#
# Marks, each its own run:
#   running  the rail's Board on web-app's team at work: a warning and a
#            decision on the board, a round of questions answered, a question
#            to another sandbox answered, a screenshot waiting, two threads
#            committing, a start that failed; at the bottom a run of bare
#            changes of state (one line, x3), a decision to the whole team and
#            a long done report (green bubble)
#   picker   the team picker open: thread names, summaries, states and ages
#   long     a team with long unbroken words (a path, hashes, 200 characters
#            without a space) and a failed report (red bubble)
#   merged   the team on docs, merged (the picker in the title)
#   all      every team at once, each row with its team
#   commit   a commit opened in the Commits column, a file's diff under it
#   folded   the conversation filtered to api-routes (its row in Team), its four commits opened
#   narrow   the window at 720x480: one column, the Commits tab
#   messages the Messages filter: the lead's and threads' direct messages
#            (the lead's two in a row share one author line)
#   wide     All teams, the Messages filter, for a large window (try
#            SIZES=2000x1100): bubbles up to 720 pt (62 percent of the
#            column), a long direct message from the lead not folded
# Out: $SBXW_SHOTS/board/board-<theme>-<size>-<mark>.png and its element tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/board"; mkdir -p "$shots"
fx="$here/fixtures/board/basic.json"
build
hash=$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1]))["daemon"]["board_timelines"]["p-run"]["commits"]["list"]; print([c for c in d if c["thread"]=="api-routes"][0]["hash"])' "$fx")
group=$(python3 -c 'import json,sys; e=json.load(open(sys.argv[1]))["daemon"]["board_timelines"]["p-run"]["events"]; c=[x for x in e if x["kind"]=="commit" and x["from"]=="api-routes"]; print("commits-"+c[-1]["id"])' "$fx")
script() { # mark size
  MARK="$1" SIZE="$2" HASH="$hash" GROUP="$group" python3 - <<'PY'
import json, os
m = os.environ["MARK"]; h = os.environ["HASH"]; g = os.environ["GROUP"]
narrow = int(os.environ["SIZE"].split("x")[0]) < 1000
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="mark", name="wait:row-web-app")
step("rail-board", "board-convo")
if m in ("merged",):
    step("board-row-picker", "board-row-team-p-old")
    step("board-row-team-p-old", "board-convo")
if m == "all":
    step("board-row-picker", "board-row-team-all")
    step("board-row-team-all", "board-convo")
if m == "picker":
    step("board-row-picker", "board-picker")
if m == "long":
    step("board-row-picker", "board-row-team-p-long")
    step("board-row-team-p-long", "board-convo")
if m == "commit":
    if narrow:
        step("board-tab-commits", "board-side-commit-" + h)
    step("board-side-commit-" + h, "board-file-src/part_0.rs")
    step("board-file-src/part_0.rs", "board-diff-src/part_0.rs")
if m == "folded":
    if narrow:
        step("board-tab-notes", "board-thread-api-routes")
    step("board-thread-api-routes", "board-tab-conversation" if narrow else "board-msg-" + g)
    if narrow:
        step("board-tab-conversation", "board-msg-" + g)
        # 480 pt tall: only the commits, so their row is in view to press.
        step("board-group-commits", "board-msg-" + g)
    add(type="mark", name="hold:300")
    step("board-msg-" + g, "board-commit-" + h)
if m == "messages":
    step("board-group-messages", "board-convo")
if m == "wide":
    step("board-row-picker", "board-row-team-all")
    step("board-row-team-all", "board-convo")
    step("board-group-messages", "board-convo")
if m == "narrow":
    step("board-tab-commits", "board-side-commit-" + h)
add(type="mark", name="hold:500")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816 720x480}; do
    for m in ${MARKS:-running picker merged all long commit folded messages narrow}; do
      [ "$m" = narrow ] && [ "$size" != 720x480 ] && continue
      [ "$m" = wide ] && [ "$size" = 720x480 ] && continue
      script "$m" "$size" >"$shots/script-$m.json"
      THEME="$theme" SHOT_SIZE="$size" run_app "$shots/board-$theme-$size-$m.log" --fixture "$fx" --script "$shots/script-$m.json" \
        --snap "$shots/board-$theme-$size-$m.png" --tree "$shots/board-$theme-$size-$m.json" --at "mark:$m+0.2"
      grep -hE "gave up|no element|script:|render:" "$shots/board-$theme-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
      echo "$shots/board-$theme-$size-$m.png"
    done
  done
done
