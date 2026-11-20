#!/usr/bin/env bash
# The New thread dialog normalizing a typed name (T-37), headless, on the
# threads/new-thread fixture (no daemon):
#
#   desktop/scripts/demo-name-normalize.sh
#   THEMES=graphite SIZES=1284x816 desktop/scripts/demo-name-normalize.sh
#
# Types "Fix Login/Bug " into the empty name field: the field holds the text
# lower-cased with a trailing hyphen, and a line says "Will be created as
# fix-login-bug". Out: $SBXW_SHOTS/threads/normalize/normalize-<theme>-<size>.png
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/threads/normalize"; mkdir -p "$shots"
build
python3 - >"$shots/script.json" <<'PY'
import json
ev = [{"t": 0.3, "type": "mark", "name": "wait:tn-name"},
      {"t": 0.5, "type": "tap", "key": "tn-name"},
      {"t": 0.6, "type": "key", "name": "a", "cmd": True},
      {"t": 0.7, "type": "key", "name": "Backspace"},
      {"t": 0.8, "type": "type", "text": "Fix Login/Bug ", "every": 0.03},
      {"t": 1.6, "type": "mark", "name": "typed"}]
print(json.dumps({"events": ev}))
PY
for theme in ${THEMES:-dark graphite}; do
  for size in ${SIZES:-1284x816}; do
    THEME="$theme" SHOT_SIZE="$size" run_app "$shots/normalize-$theme-$size.log" --fixture "$here/fixtures/threads/new-thread.json" \
      --script "$shots/script.json" --snap "$shots/normalize-$theme-$size.png" --tree "$shots/normalize-$theme-$size.json" --at "mark:typed+0.3"
    echo "$shots/normalize-$theme-$size.png"
  done
done
