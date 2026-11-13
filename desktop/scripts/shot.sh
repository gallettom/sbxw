#!/usr/bin/env bash
# One command an agent can run to see what it built: build the app, run it
# on a virtual X display against the stand-in daemon, save a screenshot.
#
#   desktop/scripts/shot.sh [out.png] [--type "text to type"] [--wait seconds]
#
# SBXW_DAEMON + SBXW_TOKEN use a real daemon; SBXW_SANDBOX picks the
# terminal's sandbox; SBXW_SCREEN another screen file.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
out="$here/shots/shot.png"
type_text=""
wait_s=3
while [ $# -gt 0 ]; do
  case "$1" in
    --type) type_text="$2"; shift 2 ;;
    --wait) wait_s="$2"; shift 2 ;;
    *) out="$1"; shift ;;
  esac
done
start_app "$(dirname "$out")" ${SBXW_SCREEN:+--screen "$SBXW_SCREEN"}
[ -n "$type_text" ] && type_line "$type_text"
sleep "$wait_s"
snap "$out"
