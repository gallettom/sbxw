#!/usr/bin/env bash
# The configure views out of Setup (MCP servers, Skills, Kits; overlays over
# the pane area since views2), headless,
# from their fixtures (no daemon), in the whole window at 1284x816 and
# 720x480: each panel with content (basic), empty, and with something
# waiting; Setup after the move (from Settings in the rail); a sandbox's
# Manage dialog with its three entries; the panels' entries (the rail's
# configure zone once it takes features' entries, the top bar before).
#
#   desktop/scripts/demo-configviews.sh
#
# Out: $SBXW_SHOTS/configviews/<view>-<state>-<size>.png, with the element
# tree beside each; fails if a tree holds a secret value or a panel is not
# drawn.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/configviews"; mkdir -p "$shots"
build
sizes=(1284x816 720x480)

snap() { # <name> <size> <fixture> [script]
  local name="$1" size="$2" fx="$3" script="${4:-}"
  local args=(--fixture "$fx" --snap "$shots/$name-$size.png" --tree "$shots/$name-$size.tree.json")
  if [ -n "$script" ]; then args+=(--script "$script" --at "mark:end+0.3"); else args+=(--at 0.8); fi
  SHOT_SIZE="$size" run_app "$shots/$name-$size.log" "${args[@]}"
  echo "$shots/$name-$size.png"
}

for feat in mcp skills kits; do
  for st in basic empty waiting; do
    for sz in "${sizes[@]}"; do
      snap "$feat-$st" "$sz" "$here/fixtures/$feat/$st.json"
      grep -q "\"overlay-$feat\"" "$shots/$feat-$st-$sz.tree.json" || { echo "FAIL: $feat overlay not drawn ($st, $sz)" >&2; exit 1; }
    done
  done
done

# Setup after the move, and the Manage dialog's entries.
cat >"$shots/setup.script.json" <<'JSON'
{"events": [{"t": 0.3, "type": "tap", "key": "rail-setup"}, {"t": 0.8, "type": "mark", "name": "end"}]}
JSON
cat >"$shots/manage.script.json" <<'JSON'
{"events": [{"t": 0.3, "type": "tap", "key": "extra-sandbox-docs"}, {"t": 0.6, "type": "tap", "key": "extra-sandbox-docs-more"}, {"t": 1.1, "type": "mark", "name": "end"}]}
JSON
for sz in "${sizes[@]}"; do
  snap setup-after "$sz" "$here/fixtures/setup/full.json" "$shots/setup.script.json"
  grep -q '"tab-kits"\|"tab-mcp"\|"tab-skills"' "$shots/setup-after-$sz.tree.json" && { echo "FAIL: Setup still lists a moved tab" >&2; exit 1; }
done
for sz in "${sizes[@]}"; do
  snap manage "$sz" "$here/fixtures/setup/full.json" "$shots/manage.script.json"
  grep -q '"sa-kits"' "$shots/manage-$sz.tree.json" || { echo "FAIL: the Manage dialog has no Kits entry ($sz)" >&2; exit 1; }
done

if grep -lE 'ghp_|sk-[A-Za-z0-9]|tok-[A-Z]' "$shots"/*.tree.json "$shots"/*.log 2>/dev/null; then
  echo "FAIL: a secret value in a tree or a log" >&2; exit 1
fi
echo "no secret value in any tree or log: ok"
