#!/usr/bin/env bash
# The file explorer over the pane area, headless, against the stand-in with
# fixtures/files/standin.json, its README and app.ts made longer here (a
# README with headings and links, a code file of 60 lines):
#
#   desktop/scripts/demo-explorer.sh
#   MARKS="markdown" SIZES=720x480 desktop/scripts/demo-explorer.sh
#
# Marks: `markdown` (opened from web-app's pane bar Project: filtered to
# web-app, the README read with its outline), `section` (an outline entry
# pressed: that section, Whole file), `code` (src/app.ts with its line
# numbers; following a link inside is covered by the unit tests, a script
# cannot press a link inside a `markdown` element),
# `rail` (the rail's Files: the sandbox used last), `home` (the Home
# location: the agent's home in the sandbox), `tree` (a folder opened in
# place), `keys` (Up then Right from a file: the folder above opens in
# place), `upload` (the header's Upload: its card).
# Out: $SBXW_SHOTS/explorer/explorer-<size>-<mark>.png and its tree.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/explorer"; mkdir -p "$shots"
build
python3 - "$here/fixtures/files/standin.json" "$shots/standin.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
c = d["daemon"]["files_content"]
c["/Users/me/web/README.md"] = """# web-app

The storefront: product pages, the cart and the checkout. This file is long
enough to show the reading measure: lines stop at about seventy characters,
in the prose face, whatever the width of the window.

## Run it

```sh
npm install
npm start
```

The app listens on port 3000. See [the entry point](src/app.ts) and the
[[CHANGELOG]] for what changed; the design notes are on
[the team site](https://example.org/web).

## Test it

- `npm test` runs the unit tests.
- `npm run e2e` drives a browser.

## Deploy

Merges to main deploy to staging. **Production** needs a tag.
"""
c["/Users/me/web/src/app.ts"] = "".join(f"export const line{i} = {i}; // a line of the code file, number {i}\n" for i in range(1, 61))
for e in d["daemon"]["files_dir"]["entries"]:
    if e["name"] == "README.md": e["size"] = len(c["/Users/me/web/README.md"])
for e in d["daemon"]["files_tree"]["/Users/me/web/src"]["entries"]:
    if e["name"] == "app.ts": e["size"] = len(c["/Users/me/web/src/app.ts"])
json.dump(d, open(sys.argv[2], "w"))
PY
script() {
  MARK="$1" SIZE="$2" python3 - <<'PY'
import json, os
m = os.environ["MARK"]
narrow = int(os.environ["SIZE"].split("x")[0]) < 1000
ev = []; t = 0.6
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def step(key, until):
    add(type="tap", key=key); add(type="mark", name="wait:" + until)
add(type="tap", key="row-web-app")
add(type="mark", name="wait:pane-1-bar")
def project():
    # Narrow, the pane bar's Project is in its "⋯" menu.
    if narrow:
        step("pane-1-more", "pane-1-x-project")
    step("pane-1-x-project", "files-row-report.md")
def loc(path, until):
    # A location: a visible list when wide, a menu when narrow (views2).
    if narrow:
        step("files-locations", "files-locations-" + path)
        step("files-locations-" + path, until)
    else:
        step("files-loc-" + path, until)
WS = "/Users/me/web"
if m == "rail":
    project()
    add(type="key", name="Escape"); add(type="mark", name="wait-gone:overlay-files")
    step("rail-files", "files-row-report.md")
elif m in ("home", "tree", "upload", "keys"):
    project()
    loc("/home/agent", "files-row-/home/agent/scratch")
    if m == "tree":
        # A folder opened in place, its file under it.
        step("files-row-/home/agent/scratch", "files-row-/home/agent/scratch/draft.txt")
    if m == "keys":
        # Down twice, Right: the keyboard opens the folder in place.
        if narrow:
            # Narrow, a file pressed shows the reader alone: the keys from the
            # list with nothing focused (the explorer's own cursor).
            add(type="key", name="Down"); add(type="key", name="Right")
        else:
            add(type="tap", key="files-row-/home/agent/notes.md"); add(type="mark", name="wait:files-preview-markdown")
            add(type="key", name="Up"); add(type="key", name="Right")
        add(type="mark", name="wait:files-row-/home/agent/scratch/draft.txt")
    if m == "upload":
        step("files-upload", "files-upload-pick")
else:
    project()
    if m == "code":
        loc(WS, "files-row-/Users/me/web/src")
        step("files-row-/Users/me/web/src", "files-row-/Users/me/web/src/app.ts")
        step("files-row-/Users/me/web/src/app.ts", "files-preview-text")
    elif narrow:
        # One pane at a time: the README's row (filtered to, the list is long
        # at 480 pt) shows the reader.
        loc(WS, "files-row-/Users/me/web/README.md")
        add(type="tap", key="files-filter"); add(type="type", text="READ", every=0.02); t += 0.4
        step("files-row-/Users/me/web/README.md", "files-outline")
    else:
        loc(WS, "files-outline")
    if m == "section":
        if narrow:
            # Narrow, the outline is a menu above the text.
            step("files-outline", "files-outline-1")
        step("files-outline-1", "files-whole")
add(type="mark", name="hold:400")
add(type="mark", name=m)
add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}
for size in ${SIZES:-1284x816 720x480}; do
  for m in ${MARKS:-markdown section code rail home tree keys upload}; do
    script "$m" "$size" >"$shots/script-$m.json"
    start_daemon "$shots/standin-$size-$m" --fixture "$shots/standin.json"
    SHOT_SIZE="$size" run_app "$shots/explorer-$size-$m.log" --script "$shots/script-$m.json" \
      --snap "$shots/explorer-$size-$m.png" --tree "$shots/explorer-$size-$m.json" --at "mark:$m+0.2"
    cleanup; pids=()
    grep -hE "gave up|no element|script:" "$shots/explorer-$size-$m.log" | sed 's/^\[[^]]*\] //' || true
    echo "$shots/explorer-$size-$m.png"
  done
done
