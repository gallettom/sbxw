# Shared by shot.sh and demo-reload.sh: build, start the stand-in daemon
# (unless SBXW_DAEMON/SBXW_TOKEN name a real one), then `run_app` runs the
# app headless through Rune's own flags (--snap, --tree, --script, --strip):
# no display, no Xvfb, no xdotool. Rune renders offscreen with wgpu; with no
# GPU, Mesa's software Vulkan driver (mesa-vulkan-drivers, llvmpipe) does it.

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
cargo=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
profile=${SBXW_PROFILE:-debug}
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

# start_daemon <logdir>: sets $daemon and $token.
start_daemon() {
  local logs="$1"
  mkdir -p "$logs"
  local flag=(); [ "$profile" = release ] && flag=(--release)
  (cd "$here" && "$cargo" build --quiet --bins "${flag[@]}")
  bin="$CARGO_TARGET_DIR/$profile"
  if [ -n "${SBXW_DAEMON:-}" ]; then
    daemon="$SBXW_DAEMON"; token="${SBXW_TOKEN:?SBXW_TOKEN}"
    return
  fi
  local port
  port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
  daemon="127.0.0.1:$port"; token="shot-$RANDOM$RANDOM"
  "$bin/standin" "$daemon" "$token" >"$logs/standin.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done
}

# run_app <logfile> [rune and app args...]
run_app() {
  local log="$1"; shift
  env -u DISPLAY -u WAYLAND_DISPLAY "$bin/sbxw-desktop" --daemon "$daemon" --token "$token" \
    --sandbox "${SBXW_SANDBOX:-web-app}" --size "${SHOT_SIZE:-1280x800}" --scale "${SHOT_SCALE:-1}" "$@" >"$log" 2>&1 \
    || { echo "app failed; see $log" >&2; tail -5 "$log" >&2; return 1; }
}
