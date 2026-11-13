# Shared by shot.sh and demo-reload.sh: build, start a virtual X display, the
# stand-in daemon (unless SBXW_DAEMON/SBXW_TOKEN name a real one) and the app.
# After `start_app <logdir> [app args...]`: $DISPLAY, $win (the window id).
# Needs: Xvfb, xdotool, ImageMagick (import), mesa-vulkan-drivers (llvmpipe).

here=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$here/target}"
cargo=${CARGO:-$(command -v cargo || echo "$HOME/.cargo/bin/cargo")}
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

start_app() {
  local logs="$1"; shift
  mkdir -p "$logs"
  (cd "$here" && "$cargo" build --quiet --bins)
  local bin="$CARGO_TARGET_DIR/debug"

  local d=90
  while [ -e "/tmp/.X11-unix/X$d" ] || [ -e "/tmp/.X$d-lock" ]; do d=$((d + 1)); done
  Xvfb ":$d" -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
  pids+=($!)
  export DISPLAY=":$d"
  for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done

  local daemon token
  if [ -n "${SBXW_DAEMON:-}" ]; then
    daemon="$SBXW_DAEMON"; token="${SBXW_TOKEN:?SBXW_TOKEN}"
  else
    local port=$((7400 + d))
    daemon="127.0.0.1:$port"; token="shot-$RANDOM$RANDOM"
    "$bin/standin" "$daemon" "$token" >"$logs/standin.log" 2>&1 &
    pids+=($!)
    for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null && break; sleep 0.1; done
  fi

  # GPUI picks Wayland when WAYLAND_DISPLAY is set; force X11 here.
  env -u WAYLAND_DISPLAY "$bin/sbxw-desktop" --daemon "$daemon" --token "$token" \
    --sandbox "${SBXW_SANDBOX:-web-app}" "$@" >"$logs/app.log" 2>&1 &
  pids+=($!)

  win=""
  for _ in $(seq 100); do
    win=$(xdotool search --onlyvisible --name "sbxw desktop" 2>/dev/null | head -1 || true)
    [ -n "$win" ] && break
    sleep 0.1
  done
  [ -n "$win" ] || { echo "no window; see $logs/app.log" >&2; return 1; }
  # GPUI 0.2.2 maps its window before it registers it, so with no window
  # manager the only MapNotify is lost and the frame loop never starts.
  # Map it again once it is registered.
  sleep 1
  xdotool windowunmap --sync "$win"
  xdotool windowmap --sync "$win"
  sleep 1
}

# Shifted characters typed through XTEST arrive unshifted (see SPIKE.md):
# stick to lower case, digits and unshifted punctuation.
type_line() {
  xdotool type --delay "${SHOT_DELAY:-20}" --window "$win" "$1" 2>/dev/null
  xdotool key --window "$win" Return 2>/dev/null
}

snap() { import -window root "$1"; echo "$1"; }
