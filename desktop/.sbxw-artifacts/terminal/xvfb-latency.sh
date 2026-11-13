#!/usr/bin/env bash
# Key-to-echo latency in a real Rune window under Xvfb: xdotool types N keys
# (XTEST), term-demo logs, for each, the time from the key leaving the UI to
# the first output byte reaching the UI thread (TerminalState::echo_ms).
#   bash desktop/.sbxw-artifacts/terminal/xvfb-latency.sh [N] [ms between keys]
set -uo pipefail
cd "$(dirname "$0")/../.."
n=${1:-100}; gap=${2:-40}
out=shots/term; mkdir -p $out
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
./target/release/standin 127.0.0.1:$port tok > $out/x-standin.log 2>&1 & SP=$!
Xvfb :97 -screen 0 1280x800x24 >/dev/null 2>&1 & XP=$!
sleep 1.5
env -u WAYLAND_DISPLAY DISPLAY=:97 RUST_LOG=info,sbxw_desktop::terminal=debug ./target/release/${BIN:-term-demo} --daemon 127.0.0.1:$port --token tok ${DEMO_ARGS:-} > $out/x-demo.log 2>&1 & DP=$!
for _ in $(seq 50); do W=$(DISPLAY=:97 xdotool search --name "${TITLE:-sbxw terminal}" 2>/dev/null | head -1); [ -n "$W" ] && break; sleep 0.2; done
sleep 2
DISPLAY=:97 xdotool windowfocus "$W" 2>/dev/null
for i in $(seq 1 "$n"); do DISPLAY=:97 xdotool key a; sleep "$(awk -v g="$gap" 'BEGIN{printf "%.3f", g/1000}')"; done
sleep 0.5
DISPLAY=:97 xwd -root -silent > $out/x-window.xwd 2>/dev/null
kill $DP $XP $SP 2>/dev/null
grep "key to echo" $out/x-demo.log | sed 's/.*echo: //; s/ ms//' | sort -n | awk '{a[NR]=$1; s+=$1} END {if (NR) print "key to echo (window, Xvfb): n",NR,"min",a[1],"p50",a[int((NR+1)/2)],"p90",a[int(NR*0.9)],"max",a[NR],"mean",s/NR; else print "no samples"}'
