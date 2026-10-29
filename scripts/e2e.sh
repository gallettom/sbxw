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

c="$HOME/.sbxw/cache"
grep "create claude $t/ws/neos" "$t/state/calls.log" | grep -q -- " $c --kit $HOME/.sbxw/kits/codemap .*-e SBXW_CACHE=$c -e SBXW_CACHE_FILL=1" || die "the first sandbox fills the cache"
"$bin" cache | grep -q "amd64-bookworm .*filled, expires" || die "sbxw cache shows the filled flavour"
ok "bundled kits resolved; the first sandbox fills the kit cache"

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

ui "http://127.0.0.1:$UI/api/sandboxes/neos/policy" | grep -q '"resource":"pastebin.com"' || die "blocked hosts from the JSON log"
ui -X POST -H 'content-type: application/json' -d '{}' "http://127.0.0.1:$UI/api/policy/refresh" | grep -q '"ok":true' || die "policy refresh"
grep -q "^policy refresh" "$t/state/calls.log" || die "policy refresh reached sbx"
ui "http://127.0.0.1:$UI/api/mcp" | grep -q '"name":"github"' || die "mcp list"
ui -X POST -H 'content-type: application/json' -d '{"spec":{"name":"acme","kind":"remote","url":"https://mcp.acme.test/mcp","headers":["Authorization: Bearer ${api-key}"]},"secrets":{"api-key":"s3cret"}}' "http://127.0.0.1:$UI/api/mcp" | grep -q '"ok":true' || die "mcp add"
grep -q "^mcp add acme --url=https://mcp.acme.test/mcp --header=Authorization: Bearer \${api-key} --skip-auth" "$t/state/calls.log" || die "mcp add argv"
grep -q "^secret set mcp:acme:api-key" "$t/state/calls.log" || die "mcp header secret"
! grep -q "s3cret" "$t/state/calls.log" || die "a secret reached argv"
ui -X POST -H 'content-type: application/json' -d '{"spec":{"name":"acme","kind":"stdio","command":"npx","args":["-y","pkg"]},"replace":"acme"}' "http://127.0.0.1:$UI/api/mcp" | grep -q '"ok":true' || die "mcp edit"
grep -q "^mcp rm acme" "$t/state/calls.log" && grep -q "^mcp add acme --command=npx --args=-y,pkg" "$t/state/calls.log" || die "mcp edit argv"
ui -X POST -H 'content-type: application/json' -d '{"spec":{"name":"--force","kind":"remote","url":"x"}}' "http://127.0.0.1:$UI/api/mcp" | grep -q '"ok":false' || die "mcp bad name refused"
ui "http://127.0.0.1:$UI/api/mcp/acme" | grep -q 'Name: acme' || die "mcp inspect"
ui -X POST -H 'content-type: application/json' -d '{"server":"github"}' "http://127.0.0.1:$UI/api/sandboxes/neos/mcp/unload" | grep -q '"ok":true' || die "mcp unload"
grep -q "^mcp unload github --sandbox neos" "$t/state/calls.log" || die "mcp unload argv"
ui -X POST -H 'content-type: application/json' -d '{"server":"github"}' "http://127.0.0.1:$UI/api/sandboxes/neos/mcp/load" | grep -q '"ok":true' || die "mcp load"
grep -q "^mcp load github --sandbox neos" "$t/state/calls.log" || die "mcp load argv"
ui "http://127.0.0.1:$UI/api/sandboxes" | grep -q '"detached":false' || die "sandbox list carries the JSON fields"
ok "network panel: blocked hosts, policy refresh, MCP add, edit, inspect, load and unload"

"$bin" doctor > "$t/doctor.log" 2>&1 || { cat "$t/doctor.log"; die "doctor"; }
grep -q "✓ daemon" "$t/doctor.log" && grep -q "! global network policy — not initialized" "$t/doctor.log" || { cat "$t/doctor.log"; die "doctor output"; }
ok "doctor checks sbxw and summarises sbx diagnose"


"$bin" up solo "$t/ws/neos" --no-web < /dev/null > "$t/solo.log" 2>&1 || { cat "$t/solo.log"; die "up --no-web"; }
grep -q "claude (fake) in solo" "$t/solo.log" || { cat "$t/solo.log"; die "--no-web attaches the agent here"; }
token2=$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1])).token)' "$HOME/.sbxw/state/sandboxes/solo.json")
code=$(curl -s -o /dev/null -w '%{http_code}' -X POST -H "authorization: Bearer $token2" -H 'content-type: application/json' \
  -d '{"hook_event_name":"SessionStart","session_id":"s2"}' "http://127.0.0.1:$AGENT/api/hook")
[ "$code" = 200 ] || die "a token the CLI minted should be accepted by the running daemon, got $code"
grep "create claude $t/ws/neos --name solo" "$t/state/calls.log" | grep -q -- "$HOME/.sbxw/cache:ro" || die "a later sandbox reads the cache"
ok "--no-web runs the pipeline here and attaches; the daemon accepts its token"

# Top-level key: appended, it would land in the last [[ports]] table.
{ echo 'template = "auto"'; cat "$t/ws/sbxw.toml"; } > "$t/toml" && mv "$t/toml" "$t/ws/sbxw.toml"
"$bin" up tpl "$t/ws/neos" --rebuild-template > "$t/tpl.log" 2>&1 || { cat "$t/tpl.log"; die "up --rebuild-template"; }
grep -q "^create claude $t/ws/neos --name tpl " "$t/state/calls.log" && ! grep "^create claude $t/ws/neos --name tpl " "$t/state/calls.log" | grep -q -- --template || die "a rebuild creates from sbx's image"
grep -q "^exec tpl -- rm -f /home/agent/.sbxw/token" "$t/state/calls.log" || { cat "$t/tpl.log"; grep tpl "$t/state/calls.log"; die "secrets taken out before the snapshot"; }
tag=$(grep -o "^template save tpl sbxw-auto:[^ ]*" "$t/state/calls.log" | awk '{print $4}')
[ -n "$tag" ] && grep -q "\"tag\": \"$tag\"" "$HOME/.sbxw/state/templates.json" || die "the template is saved and recorded"
"$bin" up tpl2 "$t/ws/neos" > /dev/null 2>&1 || die "up from the template"
grep -q "^create claude $t/ws/neos --name tpl2 --template $tag" "$t/state/calls.log" || die "the next sandbox starts from it"
! grep "^create claude $t/ws/neos --name tpl2 " "$t/state/calls.log" | grep -q "/.sbxw/cache" || die "a sandbox from a template does not mount the kit cache"
"$bin" up tpl2 "$t/ws/neos" > /dev/null 2>&1 || die "re-up"
[ "$(grep -c "^policy ls tpl2 --wide" "$t/state/calls.log")" = 0 ] || die "a sandbox this sbxw created is never searched for the old UI-port rule"
grep -q "^kit add tpl2" "$t/state/calls.log" && die "a re-up re-applies no kit sbxw recorded"
"$bin" up tpl2 "$t/ws/neos" --rebuild-template > "$t/tpl2.log" 2>&1 || die "re-up with --rebuild-template"
grep -q "already exists — a template is only rebuilt from a sandbox being created" "$t/tpl2.log" || { cat "$t/tpl2.log"; die "a rebuild asked of an existing sandbox says why not"; }
ok "template = auto: rebuilt from a new sandbox without its secrets, then used"

# The web UI's Templates dialog: list, save one from a sandbox, remove both.
post() { ui -X POST -H 'content-type: application/json' -d "$2" "http://127.0.0.1:$UI$1"; }
ui "http://127.0.0.1:$UI/api/templates" > "$t/tpls.json"
grep -q "\"tag\":\"$tag\"" "$t/tpls.json" && grep -q '"auto":{' "$t/tpls.json" || die "the automatic template is listed as one"
post /api/templates/save '{"sandbox":"tpl2","tag":"me/base:1"}' | grep -q '"ok":true' || die "save a template from the UI"
grep -q "^template save tpl2 me/base:1" "$t/state/calls.log" || die "the UI's save reaches sbx"
ui "http://127.0.0.1:$UI/api/templates" | grep -q '"tag":"me/base:1"' || die "the saved template is listed"
post /api/templates/rm '{"tag":"me/base:1"}' | grep -q '"ok":true' || die "remove a template from the UI"
post /api/templates/rm "{\"tag\":\"$tag\"}" | grep -q '"ok":true' || die "remove the automatic template"
! grep -q "\"tag\": \"$tag\"" "$HOME/.sbxw/state/templates.json" || die "removing an automatic template forgets its record"
post /api/templates/save '{"sandbox":"tpl2","tag":"--force"}' | grep -q '"ok":false' || die "a tag that reads as a flag is refused"
ok "Templates dialog: list, save without secrets, remove"

"$bin" rm neos >/dev/null || die "rm"
[ ! -e "$HOME/.sbxw/state/sandboxes/neos.json" ] || die "rm forgets the record"
ok "rm removes the sandbox and forgets its record"

"$bin" down >/dev/null || die "down"
sleep 0.5
! curl -s -o /dev/null "http://127.0.0.1:$UI/api/health" || die "the daemon should be gone"
ok "down stops the daemon"

echo "e2e: $pass checks passed"
