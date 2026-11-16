#!/usr/bin/env bash
# The sandbox's browser, headless, against the stand-in daemon (no sbx, no
# Chromium): the stand-in plays `browser.mjs serve` (src/feat/browser/
# standin.rs): pictures drawn and JPEG-encoded while the page animates, the
# address, two refused hosts, two malformed frames on purpose, and a picture
# at once for every click or key.
#
#   desktop/scripts/demo-browser.sh             # the pictures, then the measurement
#   ONLY=shots desktop/scripts/demo-browser.sh  # or ONLY=measure
#
# The pictures (one run per mark, each against a fresh stand-in):
#   1-live      web-app's terminal in pane 1, Browser on its pane bar: the
#               panel docks on the right, pictures arrive;
#   2-input     a click on the page (the stand-in draws a red box where it
#               landed), "hello" typed into the page (a bar of 5);
#   3-allow-confirm  Allow on fonts.googleapis.com: the CORE's dialog;
#   4-allowed   confirmed: the chip goes, "1 host allowed from here";
#   5-nav       an address typed in the bar and sent with Enter (to the
#               sandbox's browser, never opened here): anim.example, a page
#               that animates;
#   6-settle    Close browser: the question about the hosts allowed from here;
#   7-end       Remove them: a notice, the panel gone.
# The measurement: release build, the stand-in at 15 and at 30 pictures a
# second of 1280x800 (SBXW_STANDIN_BROWSER_FPS), 6 s of real time on the
# animating page, then 3 s of virtual time drawn as fast as the CPU allows;
# prints the frames per second the app decoded, input-to-picture latency,
# the app's CPU, and Rune's own frame cost.
# Output: $SBXW_SHOTS/browser/.
set -euo pipefail
source "$(dirname "$0")/lib.sh"
shots="$shots_root/browser"; mkdir -p "$shots"
only=${ONLY:-all}

# What happens on the way to each mark. Headless note: a key that reaches a
# terminal and gets no echo makes every later headless frame wait 1.5 s for
# it (shell.rs, `poll_tx` settle): the address bar is cleared with Cmd/Ctrl+A
# as `cmd`, which the input takes, not `ctrl`, which went to the terminal.
script() {
python3 - "$1" <<'PY'
import json, sys
mode = sys.argv[1]
ev = []; t = 0.3
def add(**e):
    global t
    e["t"] = round(t, 2); ev.append(e); t += 0.25
def mark(n, hold=0):
    if hold: add(type="mark", name=f"hold:{hold}")
    add(type="mark", name=n)
def address(text):
    global t
    add(type="tap", key="browser-url")
    add(type="key", name="a", cmd=True)
    add(type="type", text=text, every=0.03); t += 0.03 * len(text)
    add(type="key", name="Enter")
add(type="tap", key="row-web-app")
mark("terminal", 400)
add(type="tap", key="pane-1-x-browser")
if mode == "measure":
    mark("open", 1500)
    address("anim.example/")
    mark("streaming", 6000)
    # Drawn as fast as the CPU allows: 3 s of virtual time, no hold.
    t += 3.0
    add(type="tap", key="surface-browser")
    mark("measured", 300)
    add(type="mark", name="end")
else:
    mark("1-live", 2500)
    add(type="tap", key="surface-browser")
    mark("click", 300)
    add(type="type", text="hello", every=0.05); t += 0.3
    mark("2-input", 600)
    add(type="tap", key="browser-allow-fonts.googleapis.com")
    mark("3-allow-confirm", 300)
    add(type="tap", key="cf-confirm")
    mark("4-allowed", 800)
    address("anim.example/")
    mark("5-nav", 1500)
    add(type="tap", key="browser-close")
    mark("6-settle", 1200)
    add(type="tap", key="browser-settle-remove")
    mark("7-end", 800)
    add(type="mark", name="end")
print(json.dumps({"events": ev}))
PY
}

if [ "$only" != measure ]; then
  script shots >"$shots/script.json"
  # Pictures, not speed: a few pictures a second keep the headless runs short.
  export SBXW_STANDIN_BROWSER_FPS=${SBXW_STANDIN_BROWSER_FPS:-5}
  marks=$(python3 -c 'import json,sys; print(" ".join(e["name"] for e in json.load(open(sys.argv[1]))["events"] if e["type"]=="mark" and e["name"][0].isdigit()))' "$shots/script.json")
  for m in $marks; do
    start_daemon "$shots" --fixture "$here/fixtures/browser/standin.json"
    run_app "$shots/app-$m.log" --script "$shots/script.json" --snap "$shots/$m.png" --at "mark:$m+0.2" --tree "$shots/$m.json"
    cp "$shots/standin.log" "$shots/standin-$m.log"
    cleanup; pids=()
    echo "$shots/$m.png"
  done
  unset SBXW_STANDIN_BROWSER_FPS
  echo "--- what the app and the stand-in said (7-end run)"
  grep -hE "intent from browser|browser: .*(input to picture)|frame refused|Not removed|removed from" "$shots/app-7-end.log" | sed 's/^\[[^]]*\] //' | cut -c1-160 | head -20
  grep -h "standin: browser" "$shots/standin-7-end.log" | grep -v "mouseMoved" | cut -c1-160 | head -20
fi

if [ "$only" != shots ]; then
  profile=release  # lib.sh read SBXW_PROFILE when sourced
  script measure >"$shots/measure.json"
  for fps in 15 30; do
    export SBXW_STANDIN_BROWSER_FPS=$fps
    start_daemon "$shots" --fixture "$here/fixtures/browser/standin.json"
    # The app's own CPU, user and system, from the kernel's accounting of the child.
    python3 - "$bin/sbxw" "$daemon" "$token" "$shots/measure.json" "$shots/measure-$fps.log" "$shots/measure-$fps.png" <<'PY'
import os, resource, subprocess, sys, time
exe, daemon, token, script, log, png = sys.argv[1:]
env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "WAYLAND_DISPLAY")}
t = time.time()
with open(log, "w") as out:
    subprocess.run([exe, "--daemon", daemon, "--token", token, "--size", "1280x800", "--scale", "1", "--script", script, "--snap", png, "--at", "mark:measured+0.1"], stdout=out, stderr=subprocess.STDOUT, env=env, check=True)
wall = time.time() - t
r = resource.getrusage(resource.RUSAGE_CHILDREN)
print(f"app run: {wall:.1f} s wall, {r.ru_utime:.1f} s user + {r.ru_stime:.1f} s system CPU ({(r.ru_utime + r.ru_stime) / wall * 100:.0f}% of one core on average), max RSS {r.ru_maxrss // 1024} MB")
PY
    cp "$shots/standin.log" "$shots/standin-measure-$fps.log"
    cleanup; pids=()
    echo "--- stand-in at $fps pictures a second (release, headless)"
    grep -hE "browser: .*frames/s|^rune:" "$shots/measure-$fps.log" | sed 's/^\[[^]]*\] //'
    grep -h "browser: input to picture" "$shots/measure-$fps.log" | sed 's/.*picture //' | sort -n | python3 -c 'import sys; v=[int(x.split()[0]) for x in sys.stdin]; v and print(f"input to picture: {len(v)} samples, median {v[len(v)//2]} ms, max {max(v)} ms")'
    grep -h "frames sent" "$shots/standin-measure-$fps.log" | tail -1
  done
fi
