#!/usr/bin/env bash
# The sbx 0.51 migration, against a REAL sbx on this machine: what the fake
# sbx in e2e.sh cannot tell — that the v3 kits build and compose with 0.51's
# `claude` workload, that sbxw's base kit gets Node.js into it, that `kit add`
# takes a v3 kit on an existing sandbox, and what 0.51 says when it refuses.
#
#   scripts/sbx051-smoke.sh            phase A only: sbx + this checkout's kits
#   scripts/sbx051-smoke.sh --sbxw     phase B too: a full `sbxw up` with this
#                                      branch's binary (stops your sbxw daemon)
#   scripts/sbx051-smoke.sh --keep     leave the sandboxes for a look around
#
# Creates sandboxes named sbx051-smoke-*, on throwaway workspaces under
# $TMPDIR, and removes them at the end unless --keep. Paste the whole output
# (also saved to the report file it names) back to the agent.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WITH_SBXW=0 KEEP=0
for a in "$@"; do
  case "$a" in
    --sbxw) WITH_SBXW=1 ;;
    --keep) KEEP=1 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/sbx051-smoke.XXXXXX")"
REPORT="$WORK/report.txt"
exec > >(tee "$REPORT") 2>&1
A=sbx051-smoke-a B=sbx051-smoke-b C=sbx051-smoke-v2
pass=0 fail=0
ok()   { pass=$((pass + 1)); printf '  ✓ %s\n' "$*"; }
bad()  { fail=$((fail + 1)); printf '  ✗ %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
inbox() { sbx exec "$A" -- bash -lc "$1" 2>&1; }

cleanup() {
  [ "$KEEP" = 1 ] && { echo; echo "kept: $A $B $C (sbx rm --force $A $B $C)"; return; }
  sbx rm --force "$A" "$B" "$C" >/dev/null 2>&1 || true
}
trap cleanup EXIT

step "sbx"
v="$(sbx --version 2>&1)"; info "$v"
case "$v" in *0.5[1-9]*|*0.[6-9]*|*\ 1.*) ok "sbx is 0.51 or newer" ;; *) bad "sbx is older than 0.51 — nothing below is meaningful"; exit 1 ;; esac
info "clipboard.textCopy = $(sbx settings get clipboard.textCopy 2>&1)"

step "Phase A — this checkout's kits on sbx 0.51's claude workload"
mkdir -p "$WORK/ws-a" && echo "# smoke" > "$WORK/ws-a/README.md"
KITS=(base codemap agent-skills md-to-pdf-tools retro terminal-mac)
args=()
for k in "${KITS[@]}"; do args+=(--kit "$ROOT/assets/$k"); done
start=$(date +%s)
if sbx create claude "$WORK/ws-a" --name "$A" "${args[@]}" > "$WORK/create-a.log" 2>&1; then
  ok "create with base + ${KITS[*]:1} ($(( $(date +%s) - start ))s)"
else
  bad "create failed — last lines:"; tail -n 30 "$WORK/create-a.log" | sed 's/^/      /'
  exit 1
fi
sbx exec "$A" -- true >/dev/null 2>&1 || sleep 5

out="$(inbox 'command -v node && node --version && (npm --version || echo npm-missing)')"
case "$out" in *v2[0-9]*) ok "node installed by sbxw:base: $(echo "$out" | tr '\n' ' ')" ;; *) bad "no node: $out" ;; esac
info "install log: $(inbox 'ls /var/log/sbx-kits/ 2>/dev/null | tr "\n" " "')"
inbox 'cat /var/log/sbx-kits/*base*/install-*.log 2>/dev/null | tail -n 5' | sed 's/^/      /'

for f in .claude/commands/codemap.md .claude/commands/archify.md .claude/commands/md-to-pdf.md .claude/commands/retro.md .claude/commands/mac.md .sbxw/codemap/install-hooks.mjs .sbxw/mac-tools; do
  o="$(inbox "stat -c '%U %a' /home/agent/$f")"
  case "$o" in agent\ *) ok "~/$f ($o)" ;; *) bad "~/$f: $o" ;; esac
done
o="$(inbox 'stat -c %a /home/agent/.sbxw/agent-skills/install.sh /home/agent/.sbxw/retro/digest.mjs' | tr '\n' ' ')"
case "$o" in "755 755 ") ok "scripts executable ($o)" ;; *) bad "script modes: $o" ;; esac

# Startup hooks run alongside the agent: give them a moment.
sleep 20
o="$(inbox 'node -e "const s=require(\"/home/agent/.claude/settings.json\");console.log(JSON.stringify(Object.keys(s.hooks||{})), (s.statusLine||{}).command)"')"
info "settings.json hooks / statusLine: $o"
case "$o" in *SessionStart*) ok "codemap/agent-skills startup hooks ran (SessionStart hooks present)" ;; *) bad "no SessionStart hook in settings.json" ;; esac
info "sbx's statusLine (chained by sbxw's tooling in phase B): $(echo "$o" | awk '{print $2, $3, $4}')"
o="$(inbox 'test -s /home/agent/.claude/CLAUDE.md && grep -c "" /home/agent/.claude/CLAUDE.md')"
info "~/.claude/CLAUDE.md lines: $o"

step "kit add of a v3 kit on the existing sandbox (k8s-tools)"
if sbx kit add "$A" "$ROOT/assets/k8s-tools" > "$WORK/kit-add.log" 2>&1; then
  ok "kit add accepted"
  sleep 25
  o="$(inbox 'ls /home/agent/.local/bin/ 2>&1 | tr "\n" " "')"
  case "$o" in *sbxw-install-k8s*) ok "its files are there: $o" ;; *) bad "files missing after kit add: $o" ;; esac
  o="$(inbox 'command -v node')"; [ -n "$o" ] && ok "node survived the container swap" || bad "node gone after kit add"
else
  bad "kit add refused:"; sed 's/^/      /' "$WORK/kit-add.log" | tail -n 15
fi

step "sbx inspect (what sbxw parses)"
sbx inspect "$A" 2>&1 | sed -n '1,40p' | sed 's/^/      /'
info "--format json:"; sbx inspect "$A" --format json 2>&1 | head -c 1500 | sed 's/^/      /'; echo

step "A v2 kit beside the v3 workload (sbxw refuses these itself; sbx's own wording)"
mkdir -p "$WORK/v2kit" "$WORK/ws-c"
printf 'schemaVersion: "2"\nkind: mixin\nname: v2kit\nsetup:\n  files:\n    - path: /home/agent/x\n      content: "x"\n' > "$WORK/v2kit/spec.yaml"
if sbx create claude "$WORK/ws-c" --name "$C" --kit "$WORK/v2kit" > "$WORK/create-v2.log" 2>&1; then
  bad "sbx accepted a v2 kit with the v3 claude workload (sbxw refuses it anyway)"
else
  ok "sbx refused it:"; tail -n 8 "$WORK/create-v2.log" | sed 's/^/      /'
fi

if [ "$WITH_SBXW" = 1 ]; then
  step "Phase B — sbxw up with this branch's binary"
  echo "    This stops your running sbxw daemon (closing its terminals). Enter to go on, Ctrl-C to stop."
  read -r _
  (cd "$ROOT" && cargo build --release -p sbxw >/dev/null 2>&1) && ok "built sbxw from $(git -C "$ROOT" rev-parse --short HEAD)" || { bad "cargo build failed"; exit 1; }
  SBXW="$ROOT/target/release/sbxw"
  command -v sbxw >/dev/null && sbxw down >/dev/null 2>&1
  mkdir -p "$WORK/ws-b"
  printf 'kits = ["sbxw:codemap", "sbxw:retro"]\n' > "$WORK/ws-b/sbxw.toml"
  if (cd "$WORK/ws-b" && "$SBXW" up "$B" . > "$WORK/up-b.log" 2>&1); then ok "sbxw up $B"; else bad "sbxw up failed:"; tail -n 25 "$WORK/up-b.log" | sed 's/^/      /'; fi
  grep -iE 'warn|could not|failed' "$WORK/up-b.log" | sed 's/^/      ! /' | head -n 15
  o="$(sbx exec "$B" -- bash -lc 'node -e "const s=require(\"/home/agent/.claude/settings.json\");console.log((s.statusLine||{}).command)"; cat /home/agent/.sbxw/usage-statusline.chain 2>/dev/null; test -s /home/agent/.sbxw/token && echo token-ok; grep -c sbxw-relay /home/agent/.claude.json' 2>&1 | tr '\n' ' ')"
  info "statusLine / chain / token / relay: $o"
  case "$o" in *usage-statusline.js*sbx-claude-hook*token-ok*) ok "sbxw's status line installed and sbx's chained; token in place" ;; *) bad "tooling not as expected" ;; esac
  "$SBXW" doctor 2>&1 | sed -n '1,25p' | sed 's/^/      /'
  "$SBXW" rm "$B" >/dev/null 2>&1 || true
  "$SBXW" down >/dev/null 2>&1 || true
  info "your own sbxw daemon is stopped; \`sbxw up\` starts it again"
fi

echo
echo "passed $pass, failed $fail — report: $REPORT"
[ "$fail" = 0 ]
