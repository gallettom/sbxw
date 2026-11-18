# Shared by the scripts here: build, start the stand-in daemon (unless
# SBXW_DAEMON/SBXW_TOKEN name a real one, or the run uses a fixture), then
# `run_app` runs the app headless through Rune's own flags (--snap, --tree,
# --script, --strip): no display, no Xvfb, no xdotool. Rune renders offscreen
# with wgpu; with no GPU, Mesa's software Vulkan driver (mesa-vulkan-drivers,
# llvmpipe) does it.
#
# Marks the app itself acts on, in a --script (`{"type": "mark", "name": ...}`):
#   hold:<ms>              wait that long in real time, taking in what the
#                          daemon sends (the script's clock does not wait).
#   wait:<key>[:<ms>]      wait in real time until an element with that key
#                          is drawn (a screen's key or input id, the core's
#                          dialog parts cf-*, pane bars and their extras,
#                          terminals grid-<n>), at most <ms> (10 s by default).
#                          Logs "waited N ms for <key>" or "gave up waiting
#                          for <key> after N ms"; then the script goes on, so
#                          a tap on a key that never came fails as usual.
#   wait-gone:<key>[:<ms>] the same, until no element of that key is drawn
#                          (a dialog closed, a busy line gone).
#   screen:<name>=<path>   load that file as that screen now.
# Prefer wait: before a tap on something the daemon's answer draws: a fixed
# hold is a guess that fails under load.
#
# THEME=light (or dark, or system) runs the app in that theme: any demo
# script or shot.sh re-shoots in the light theme with no change to it
# (`THEME=light desktop/scripts/demo-commits.sh`).

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
cargo=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
profile=${SBXW_PROFILE:-debug}
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
# Where scripts write their shots and logs: never into a tracked path. The
# root artifacts folder of the checkout they run in (git-ignored), or
# SBXW_SHOTS when set.
root=$(git -C "$here" rev-parse --show-toplevel 2>/dev/null || dirname "$here")
export SBXW_SHOTS="${SBXW_SHOTS:-$root/.sbxw-artifacts/desktop/shots}"
shots_root="$SBXW_SHOTS"
mkdir -p "$shots_root"
# No override folder of the person's leaks into a run: screens come from the
# binary unless a script points SBXW_DESKTOP_SCREENS somewhere on purpose.
export SBXW_DESKTOP_SCREENS="${SBXW_DESKTOP_SCREENS:-$shots_root/.no-overrides}"

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
  env -u DISPLAY -u WAYLAND_DISPLAY SBXW_DESKTOP_THEME="${THEME:-${SBXW_DESKTOP_THEME:-}}" "$bin/sbxw" "${target[@]}" \
    --size "${SHOT_SIZE:-1280x800}" --scale "${SHOT_SCALE:-1}" "$@" >"$log" 2>&1 \
    || { echo "app failed; see $log" >&2; tail -5 "$log" >&2; return 1; }
}
