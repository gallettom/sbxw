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
mkdir -p "$t/fakebin" "$t/home" "$t/ws/myapp" "$t/state" "$t/remote/bin" "$t/remote/srv/api" "$t/rstate"
cp "$here/fake-sbx" "$t/fakebin/sbx"; chmod +x "$t/fakebin/sbx"
# Remote hosts, faked as in e2e.sh: `ssh` runs commands in $t/remote, whose
# `sbx` keeps its sandboxes in $t/rstate. `gonehost` never answers, as a host
# that is switched off; `newhost` is one the UI adds, with a key to confirm.
printf '#!/bin/sh
case " $* " in *" gonehost "*) echo "ssh: connect to host gonehost port 22: No route to host" >&2; exit 255;; esac
exec "%s" "$@"
' "$here/fake-ssh" > "$t/fakebin/ssh"; chmod +x "$t/fakebin/ssh"
printf '#!/bin/sh
FAKE_SBX_STATE="%s" exec "%s" "$@"
' "$t/rstate" "$here/fake-sbx" > "$t/remote/bin/sbx"; chmod +x "$t/remote/bin/sbx"
ssh-keygen -q -t ed25519 -N '' -f "$t/hostkey" >/dev/null
printf '#!/bin/sh
for a; do h="$a"; done
echo "$h $(cut -d" " -f1,2 "%s")"
' "$t/hostkey.pub" > "$t/fakebin/ssh-keyscan"; chmod +x "$t/fakebin/ssh-keyscan"
# A thread or session runs `claude` through `sbx exec`, which the fake runs here.
printf '#!/bin/sh\necho "claude (fake) $*"\nexec sh\n' > "$t/fakebin/claude"; chmod +x "$t/fakebin/claude"
free_port() { node -e 'const s=require("net").createServer().listen(0,"127.0.0.1",()=>{console.log(s.address().port);s.close()})'; }
UI=$(free_port); AGENT=$(free_port)
export HOME="$t/home" PATH="$t/fakebin:$PATH" FAKE_SBX_STATE="$t/state" SBXW_HOSTS_FILE="$t/hosts"
export FAKE_SSH_HOME="$t/remote" FAKE_SSH_LOG="$t/ssh.log"
printf '127.0.0.1\tlocalhost\n' > "$t/hosts"
printf 'web_addr = "127.0.0.1:%s"\nagent_addr = "127.0.0.1:%s"\nnetwork_allow = []\nmonitor_cmd = ["sh"]\n' "$UI" "$AGENT" > "$t/ws/sbxw.toml"
cd "$t/ws"
"$bin" up myapp "$t/ws/myapp" >/dev/null 2>&1
# Two registered hosts: `fake` answers and has a sandbox, `gone` does not
# answer and holds the record of one, which the sidebar shows greyed.
RAGENT=$(free_port)
mkdir -p "$HOME/.ssh" "$HOME/.sbxw/state/hosts/gone/sandboxes"
printf 'fakehost %s\ngonehost %s\n' "$(cut -d' ' -f1,2 "$t/hostkey.pub")" "$(cut -d' ' -f1,2 "$t/hostkey.pub")" > "$HOME/.ssh/known_hosts"
printf '[hosts.fake]\nhost = "fakehost"\nagent_port = %s\nsbx_path = "%s"\n\n[hosts.gone]\nhost = "gonehost"\n' "$RAGENT" "$t/remote/bin/sbx" > "$HOME/.sbxw/hosts.toml"
printf '{"workspace":"/srv/old","last_status":"running"}\n' > "$HOME/.sbxw/state/hosts/gone/sandboxes/old.json"
"$bin" up fake~api --path "$t/remote/srv/api" >/dev/null 2>&1 || { echo "remote up failed"; exit 1; }
token=$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1])).token)' "$HOME/.sbxw/state/sandboxes/myapp.json")
curl -sS -X POST -H "authorization: Bearer $token" -H 'content-type: application/json' \
  -d '{"five_hour_pct":42,"seven_day_pct":17}' "http://127.0.0.1:$AGENT/api/usage" >/dev/null
node "$here/ui-smoke.mjs" "$UI" myapp "$out"
echo "screenshot: $out"
