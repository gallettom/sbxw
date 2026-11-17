#!/usr/bin/env bash
# The Setup dialog, headless, from its fixtures (no daemon).
#
#   desktop/scripts/demo-setup.sh            # every fixture state, one PNG each
#   desktop/scripts/demo-setup.sh window     # the whole window, scripted (see below)
#
# Part 1: `screen.sh setup <fixture>` with a script that presses the tab
# first, for each pair below. Out: $SBXW_SHOTS/setup/<fixture>-<tab>.png and the
# element tree beside it.
#
# Part 2 (`window`): the whole window against the fixture `full` (the
# fixture is the daemon: the feature's stand-in routes answer in memory).
# It opens Setup from the top bar's gear, stores a GitHub token through the
# core's dialog (typed into its secure field), adds a stdio MCP server
# (the host command and the acknowledgement), adds a remote host (the
# fingerprints read by the core), starts the voice download, saves a kits
# line adding sbxw:ssh-agent (acknowledged), closes Setup and presses the
# amber check on docs (a kit-add pane). One PNG
# per step in $SBXW_SHOTS/setup/window-*.png, and the app log, where the token
# must not appear.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/setup"; mkdir -p "$shots"
size="${SHOT_SIZE:-760x820}"

pairs=(
  full:secrets full:kits full:skills full:mcp full:hosts full:voice
  empty:secrets empty:kits empty:skills empty:mcp empty:hosts empty:voice
  errors:secrets errors:kits errors:mcp errors:hosts
  looking:skills looked:skills looked:mcp looked:hosts
  voice-downloading:voice voice-failed:voice no-voice:voice
)

tab_script() {
  python3 - "$1" <<'PY'
import json, sys
print(json.dumps({"events": [
  {"t": 0.2, "type": "tap", "key": "tab-" + sys.argv[1]},
  {"t": 0.5, "type": "mark", "name": "end"},
]}))
PY
}

if [ "${1:-}" != "window" ]; then
  for p in "${pairs[@]}"; do
    fx="${p%%:*}"; tab="${p##*:}"
    s="$shots/tab-$tab.json"; tab_script "$tab" >"$s"
    "$here/scripts/screen.sh" setup "$fx" --size "$size" --script "$s" \
      --out "$shots/$fx-$tab.png" | grep -v '^/' || true
    echo "$shots/$fx-$tab.png"
  done
  exit 0
fi

# ── Part 2: the whole window ──────────────────────────────────────────────
token="ghp_DEMOTOKEN0000000000000000000000"
script="$shots/window-script.json"
TOKEN="$token" python3 - >"$script" <<'PY'
import json, os
ev = []; t = 0.4
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
def typ(key, text):
    global t
    add(type="tap", key=key)
    add(type="type", every=0.01, text=text); t += 0.01 * len(text)
add(type="tap", key=os.environ.get("GEAR", "rail-setup"))
mark("opened", 400)
add(type="tap", key="github-set")
mark("token-dialog", 300)
typ("cf-secret-0", os.environ["TOKEN"])
mark("token-typed")
add(type="tap", key="cf-confirm")
mark("token-saved", 600)
add(type="tap", key="tab-mcp")
typ("mcp-name", "playwright2")
add(type="tap", key="mcp-kind"); add(type="tap", key="mcp-kind-stdio")
typ("mcp-command", "npx")
typ("mcp-args", "-y @playwright/mcp@latest")
add(type="tap", key="mcp-save")
mark("mcp-dialog", 300)
add(type="tap", key="cf-ack")
mark("mcp-acked")
add(type="tap", key="cf-confirm")
mark("mcp-added", 600)
add(type="tap", key="tab-hosts")
typ("host-name", "gpu")
add(type="tap", key="host-mode"); add(type="tap", key="host-mode-direct")
typ("host-host", "10.0.0.9")
add(type="tap", key="host-save")
mark("host-dialog", 600)
add(type="tap", key="cf-ack")
add(type="tap", key="cf-confirm")
mark("host-added", 600)
add(type="tap", key="tab-voice")
add(type="tap", key="voice-rm")
add(type="tap", key="cf-confirm")
mark("voice-removed", 500)
add(type="tap", key="voice-install")
mark("voice-dialog", 300)
add(type="tap", key="cf-confirm")
mark("voice-started", 500)
add(type="tap", key="setup-reload")
mark("voice-progress", 400)
add(type="tap", key="tab-kits")
add(type="tap", key="kit-ssh-agent")
add(type="tap", key="kits-save")
mark("kits-dialog", 300)
add(type="tap", key="cf-ack")
add(type="tap", key="cf-confirm")
mark("kits-saved", 500)
add(type="tap", key="setup-close")
add(type="tap", key="extra-sandbox-docs-kit-add")
mark("kit-pane", 500)
mark("end")
print(json.dumps({"events": ev}, indent=1))
PY
marks=(opened token-dialog token-typed token-saved mcp-dialog mcp-acked mcp-added host-dialog host-added voice-removed voice-dialog voice-started voice-progress kits-dialog kits-saved kit-pane end)
build
for m in "${marks[@]}"; do
  SHOT_SIZE="1280x860" run_app "$shots/window-$m.log" --fixture "$here/fixtures/setup/full.json" \
    --script "$script" --snap "$shots/window-$m.png" --tree "$shots/window-$m.tree.json" --at "mark:$m+0.05"
  echo "$shots/window-$m.png"
done
# The script types the token, so it holds it; nothing else may.
if grep -l "$token" "$shots"/window-*.log "$shots"/window-*.tree.json 2>/dev/null; then
  echo "FAIL: the token reached a log or a tree" >&2; exit 1
fi
echo "token in no log and no element tree: ok"
grep -hE "setup:|confirmed|intent" "$shots/window-end.log" | sed 's/^\[[^]]*\] //' | sort -u | head -40 || true
