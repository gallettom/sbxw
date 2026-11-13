# Shared by the scripts here: build, start the stand-in daemon (unless
# SBXW_DAEMON/SBXW_TOKEN name a real one, or the run uses a fixture), then
# `run_app` runs the app headless through Rune's own flags (--snap, --tree,
# --script, --strip): no display, no Xvfb, no xdotool. Rune renders offscreen
# with wgpu; with no GPU, Mesa's software Vulkan driver (mesa-vulkan-drivers,
# llvmpipe) does it.

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
cargo=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
profile=${SBXW_PROFILE:-debug}
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
# No override folder of the person's leaks into a run: screens come from the
# binary unless a script points SBXW_DESKTOP_SCREENS somewhere on purpose.
export SBXW_DESKTOP_SCREENS="${SBXW_DESKTOP_SCREENS:-$here/shots/.no-overrides}"

build() {
  local flag=(); [ "$profile" = release ] && flag=(--release)
  (cd "$here" && "$cargo" build --quiet --bins "${flag[@]}")
  bin="$CARGO_TARGET_DIR/$profile"
}

free_port() {
  python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'
}

# start_daemon <logdir> [stand-in args...]: sets $daemon and $token.
start_daemon() {
  local logs="$1"; shift
  mkdir -p "$logs"
  build
  if [ -n "${SBXW_DAEMON:-}" ]; then
    daemon="$SBXW_DAEMON"; token="${SBXW_TOKEN:?SBXW_TOKEN}"
    return
  fi
  local port; port=$(free_port)
  daemon="127.0.0.1:$port"; token="shot-$RANDOM$RANDOM"
  "$bin/standin" "$daemon" "$token" "$@" >"$logs/standin.log" 2>&1 &
  pids+=($!)
  for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done
}

# run_app <logfile> [rune and app args...]: against $daemon, or a fixture
# when the args carry --fixture.
run_app() {
  local log="$1"; shift
  local target=(--daemon "${daemon:-}" --token "${token:-}")
  case " $* " in *" --fixture "*) target=() ;; esac
  env -u DISPLAY -u WAYLAND_DISPLAY "$bin/sbxw" "${target[@]}" \
    --size "${SHOT_SIZE:-1280x800}" --scale "${SHOT_SCALE:-1}" "$@" >"$log" 2>&1 \
    || { echo "app failed; see $log" >&2; tail -5 "$log" >&2; return 1; }
}
