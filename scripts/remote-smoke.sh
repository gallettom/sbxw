#!/usr/bin/env bash
# Checks a registered remote host without creating a sandbox: `sbxw host test`
# (ssh + sbx version), then `sbxw host check` (quoting, sbx detection, the
# ControlMaster link, -L and -R, the host's disk, kit copies, git). Needs only
# ssh and an Ubuntu host with sbx installed; no KVM. EXPERIMENTAL.
#
#   scripts/remote-smoke.sh <host> [--keep]
#
# <host> is a name from `sbxw host ls`. SBXW_BIN picks another binary.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"

[ $# -ge 1 ] || { echo "usage: $0 <host> [--keep]" >&2; exit 2; }
host="$1"; shift
keep=()
for a in "$@"; do
  case "$a" in
    --keep) keep=(--keep) ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

bin="${SBXW_BIN:-${CARGO_TARGET_DIR:-$root/target}/debug/sbxw}"
if [ ! -x "$bin" ]; then
  echo "building sbxw..."
  (cd "$root" && cargo build -p sbxw)
fi

status=0
echo "== sbxw host test $host"
"$bin" host test "$host" || status=$?
if [ "$status" -eq 0 ]; then
  echo
  echo "== sbxw host check $host"
  "$bin" host check "$host" ${keep[@]+"${keep[@]}"} || status=$?
else
  echo "host test failed: fix that first (ssh access, host key, sbx on the host), then run this again." >&2
fi

cat <<'TXT'

What this did NOT check (it needs a host with KVM, so a real sandbox can run):
  1. Creating a remote sandbox:   sbxw up <host>~<name> --path ...
  2. The interactive terminal:    ssh -tt <host> sbx run ...
  3. The sandbox -> host -> this machine path of the -R tunnel (the check only
     proves host -> this machine). Run it once from inside a remote sandbox.
Until someone has done these on a real KVM host, remote hosts stay EXPERIMENTAL.
TXT
exit "$status"
