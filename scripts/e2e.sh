#!/usr/bin/env bash
# End to end: the real sbxw binary against scripts/fake-sbx, in a throwaway
# HOME, on free ports. Brings a sandbox up through CLI → daemon, then checks
# the boundaries and the main flows the way a browser, an island and an agent
# would. Touches nothing outside its temp dir: the hosts file is redirected
# (SBXW_HOSTS_FILE) and the in-sandbox tooling script is captured, never run.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
bin="${SBXW_BIN:-${CARGO_TARGET_DIR:-$root/target}/debug/sbxw}"
[ -x "$bin" ] || { echo "build first: cargo build -p sbxw"; exit 1; }

t="$(mktemp -d)"
trap '"$bin" down >/dev/null 2>&1 || true; rm -rf "$t"' EXIT
mkdir -p "$t/fakebin" "$t/home" "$t/ws/neos" "$t/state"
cp "$here/fake-sbx" "$t/fakebin/sbx"
free_port() { node -e 'const s=require("net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'; }
UI=$(free_port); AGENT=$(free_port)
export HOME="$t/home" PATH="$t/fakebin:$PATH" FAKE_SBX_STATE="$t/state" SBXW_HOSTS_FILE="$t/hosts"
unset CLAUDE_CODE_OAUTH_TOKEN CLAUDE_OAUTH_TOKEN ANTHROPIC_API_KEY
printf '127.0.0.1\tlocalhost\n' > "$t/hosts"
cat > "$t/ws/sbxw.toml" <<TOML
web_addr = "127.0.0.1:$UI"
agent_addr = "127.0.0.1:$AGENT"
network_allow = ["registry.npmjs.org"]
monitor_cmd = []

[[ports]]
alias = "neos.local"
sandbox_port = 4200
host_port = 4200
TOML
cd "$t/ws"

pass=0
ok() { pass=$((pass + 1)); printf '  ✓ %s\n' "$1"; }
die() { printf '  ✕ %s\n' "$1"; echo "--- daemon log"; tail -40 "$HOME/.sbxw/run/daemon.log" 2>/dev/null || true; exit 1; }
ui() { curl -sS -H "host: 127.0.0.1:$UI" "$@"; }

echo "sbxw e2e — UI :$UI, agent :$AGENT"
out=$("$bin" up neos "$t/ws/neos" 2>&1) || { echo "$out"; die "sbxw up failed"; }
echo "$out" | grep -q "Creating the sandbox" && echo "$out" | grep -q "'neos' is up" || { echo "$out"; die "up output"; }
ok "sbxw up started the daemon and streamed the bring-up"

grep -q "create claude $t/ws/neos --name neos" "$t/state/calls.log" || die "create argv"
grep -q -- "-p 4200:4200" "$t/state/calls.log" || die "ports at creation"
grep -q "policy allow network --sandbox neos localhost:$AGENT,registry.npmjs.org" "$t/state/calls.log" || die "policy: the agent port only"
! grep -q "localhost:$UI" "$t/state/calls.log" || die "the UI port must never be allowed"
ok "created with ports; egress allows the agent port, never the UI port"

grep -q "neos.local" "$t/hosts" && grep -q "sbxw.localhost" "$t/hosts" || die "hosts block"
ok "hosts aliases written (redirected file)"

token=$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1])).token)' "$HOME/.sbxw/state/sandboxes/neos.json")
grep -q "$token" "$t/state/tooling-neos.js" || die "token installed by the tooling script"
! grep -q "$token" "$t/state/calls.log" || die "the token leaked into an argv"
ok "agent token minted, installed over stdin, never on a command line"

ui "http://127.0.0.1:$UI/api/sandboxes" | grep -q '"name":"neos"' || die "list"
code=$(curl -s -o /dev/null -w '%{http_code}' -H "host: evil.example:$UI" "http://127.0.0.1:$UI/api/sandboxes")
[ "$code" = 403 ] || die "foreign Host should be 403, got $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -H "host: 127.0.0.1:$UI" -H "origin: https://evil.example" "http://127.0.0.1:$UI/api/sandboxes")
[ "$code" = 403 ] || die "foreign Origin should be 403, got $code"
ok "UI listener answers its own Host/Origin and refuses others"

code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{}' "http://127.0.0.1:$AGENT/api/hook")
[ "$code" = 401 ] || die "agent without token should be 401, got $code"
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' -d '{}' "http://127.0.0.1:$AGENT/api/relay/1/approve")
[ "$code" = 404 ] || die "human decisions must not exist on the agent port, got $code"
curl -sS -X POST -H "authorization: Bearer $token" -H 'content-type: application/json' \
  -d '{"hook_event_name":"SessionStart","session_id":"s1","hook_version":2,"cwd":"'"$t/ws/neos"'"}' "http://127.0.0.1:$AGENT/api/hook" >/dev/null
ui "http://127.0.0.1:$UI/api/sessions" | grep -q '"sandbox":"neos"' || die "hook reached the session state"
ok "agent listener: token required, identity from it, no human endpoints"

# Relay: the agent asks and parks; the human approves; the answer comes back.
( curl -sS -X POST -H "authorization: Bearer $token" -H 'content-type: application/json' \
    -d '{"from":"someone-else","question":"what is 6x7?","timeout":20}' "http://127.0.0.1:$AGENT/api/relay/ask" > "$t/ask.json" ) &
asker=$!
for _ in $(seq 50); do ui "http://127.0.0.1:$UI/api/relay" | grep -q '"r-1"' && break; sleep 0.1; done
ui "http://127.0.0.1:$UI/api/relay" | grep -q '"from":"neos"' || die "identity comes from the token, not the body"
ui -X POST -H 'content-type: application/json' -d '{"answer":"42"}' "http://127.0.0.1:$UI/api/relay/r-1/approve" | grep -q '"ok":true' || die "approve"
wait $asker
grep -q '"answer":"42"' "$t/ask.json" && grep -q '"state":"approved"' "$t/ask.json" || { cat "$t/ask.json"; die "the parked ask got the answer"; }
ok "relay: ask parks, a human approves, the asker gets the answer"

# Terminal: attach over WebSocket, see the fake agent, type, reconnect, replay.
cat > "$t/ws.mjs" <<'JS'
const [port, expect, send] = process.argv.slice(2);
const ws = new WebSocket(`ws://127.0.0.1:${port}/ws?sandbox=neos`);
ws.binaryType = 'arraybuffer';
let seen = '';
const done = (code) => { try { ws.close(); } catch {} ; process.exit(code); };
setTimeout(() => { console.error('timeout; saw: ' + JSON.stringify(seen)); done(1); }, 10000);
ws.onmessage = (ev) => {
  seen += typeof ev.data === 'string' ? ev.data : Buffer.from(ev.data).toString();
  if (send && seen.includes('> ') && !globalThis.sent) { globalThis.sent = 1; ws.send(Buffer.from(send + '\r')); }
  if (seen.includes(expect)) done(0);
};
JS
node "$t/ws.mjs" "$UI" "claude (fake) in neos" "typed-by-hand" || die "websocket attach"
node "$t/ws.mjs" "$UI" "GOT[typed-by-hand]" "" || die "reconnect replays the session"
ok "terminal: attach, type, reconnect with replay"

ui -X POST -H 'content-type: application/json' -d '{"name":"neos","text":"hello from the island"}' "http://127.0.0.1:$UI/api/chat/push" | grep -q '"ok":true' || die "chat push"
node "$t/ws.mjs" "$UI" "GOT[hello from the island]" "" || die "the pushed message was submitted"
ok "chat push types into the live agent and submits it"

code=$(curl -s -o /dev/null -w '%{http_code}' -H "host: 127.0.0.1:$UI" -H "origin: https://evil.example" \
  -H 'connection: upgrade' -H 'upgrade: websocket' -H 'sec-websocket-version: 13' -H 'sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==' \
  "http://127.0.0.1:$UI/ws?sandbox=neos&mode=bash")
[ "$code" = 403 ] || die "cross-site websocket should be 403, got $code"
ok "a cross-site WebSocket is refused"

ui "http://127.0.0.1:$UI/api/sandboxes/neos/ports" | grep -q '"host_port":4200' || die "ports panel"
ui "http://127.0.0.1:$UI/api/sandboxes/neos/envfile" | grep -q 'workspace: ./neos' || die "env export"
ok "ports and env-file panels read the live sandbox"

"$bin" rm neos >/dev/null || die "rm"
[ ! -e "$HOME/.sbxw/state/sandboxes/neos.json" ] || die "rm forgets the record"
ok "rm removes the sandbox and forgets its record"

"$bin" down >/dev/null || die "down"
sleep 0.5
! curl -s -o /dev/null "http://127.0.0.1:$UI/api/health" || die "the daemon should be gone"
ok "down stops the daemon"

echo "e2e: $pass checks passed"
