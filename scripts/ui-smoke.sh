#!/usr/bin/env bash
# The web UI, served by the rebuilt daemon, in headless Chromium (see
# ui-smoke.mjs). Same throwaway setup as e2e.sh. Prints where the screenshot is.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
bin="${SBXW_BIN:-${CARGO_TARGET_DIR:-$root/target}/debug/sbxw}"
out="${1:-$(mktemp -d)/ui.png}"
t="$(mktemp -d)"
trap '"$bin" down >/dev/null 2>&1 || true; rm -rf "$t"' EXIT
mkdir -p "$t/fakebin" "$t/home" "$t/ws/neos" "$t/state"
cp "$here/fake-sbx" "$t/fakebin/sbx"
# A thread or session runs `claude` through `sbx exec`, which the fake runs here.
printf '#!/bin/sh\necho "claude (fake) $*"\nexec sh\n' > "$t/fakebin/claude"; chmod +x "$t/fakebin/claude"
free_port() { node -e 'const s=require("net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'; }
UI=$(free_port); AGENT=$(free_port)
export HOME="$t/home" PATH="$t/fakebin:$PATH" FAKE_SBX_STATE="$t/state" SBXW_HOSTS_FILE="$t/hosts"
printf '127.0.0.1\tlocalhost\n' > "$t/hosts"
printf 'web_addr = "127.0.0.1:%s"\nagent_addr = "127.0.0.1:%s"\nnetwork_allow = []\nmonitor_cmd = ["sh"]\n' "$UI" "$AGENT" > "$t/ws/sbxw.toml"
cd "$t/ws"
"$bin" up neos "$t/ws/neos" >/dev/null 2>&1
token=$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1])).token)' "$HOME/.sbxw/state/sandboxes/neos.json")
curl -sS -X POST -H "authorization: Bearer $token" -H 'content-type: application/json' \
  -d '{"five_hour_pct":42,"seven_day_pct":17}' "http://127.0.0.1:$AGENT/api/usage" >/dev/null
node "$here/ui-smoke.mjs" "$UI" neos "$out"
echo "screenshot: $out"
