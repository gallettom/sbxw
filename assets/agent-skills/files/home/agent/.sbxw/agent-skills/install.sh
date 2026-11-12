#!/usr/bin/env bash
# Install the pinned agent skills, and what they run on.
# Idempotent: every step checks before it acts, and a failed step is
# reported and skipped rather than failing the container start.
# No dollar-brace anywhere: kit `content` fields reject it.
set -uo pipefail
info() { echo "  [agent-skills] $*"; }

# ── Startup marker ───────────────────────────────────────────────────
# Left when this script ends, however it ends, with the boot it ran in:
# how sbxw knows the install is over before it saves a template (a process
# list cannot tell "not started yet" from "finished"). Removed first, so a
# marker from an earlier start is never read as this one's.
SBXW_DONE="$HOME/.sbxw/startup/$(echo "$0" | tr / _).done"
mkdir -p "$HOME/.sbxw/startup" && rm -f "$SBXW_DONE"
trap 'cat /proc/sys/kernel/random/boot_id > "$SBXW_DONE" 2>/dev/null' EXIT

BASE=/home/agent/.sbxw/agent-skills
BIN=/home/agent/.local/bin
PW_BROWSERS=/home/agent/.cache/ms-playwright
ENV_FILE=/etc/sandbox-persistent.sh
mkdir -p "$BASE" "$BIN"

# ── The sbxw host cache ──────────────────────────────────────────────
# sbxw mounts ~/.sbxw/cache into the sandboxes it creates and
# names it in SBXW_CACHE: what was downloaded once (apt packages and
# lists, Chromium, the skill tarballs) is copied from there instead of fetched again.
# Only the sandbox sbxw's daemon fills it from (its own, which nobody
# works in) sees it writable, with SBXW_CACHE_FILL=1. printenv, not the
# variable: set -u, and no dollar-brace defaults in kit content.
CACHE=""; FILL=""
SBXW_CACHE="$(printenv SBXW_CACHE)"
if [ -n "$SBXW_CACHE" ] && [ -d "$SBXW_CACHE" ]; then
  CACHE="$SBXW_CACHE/$(dpkg --print-architecture 2>/dev/null)-$(. /etc/os-release 2>/dev/null; echo "$VERSION_CODENAME")"
  if [ "$(printenv SBXW_CACHE_FILL)" = 1 ]; then
    FILL=1; mkdir -p "$CACHE/apt/partial" "$CACHE/apt-lists" "$CACHE/tarballs" "$CACHE/ms-playwright"
  fi
  [ -d "$CACHE" ] || CACHE=""
fi

# ── Pins. Move one to update: the next container start fetches it. ───
ARCHIFY_REPO=tt-a1i/archify
ARCHIFY_REV=73aaa0696e8f72c232ea710e6fa94fd953f3e773   # v3.0.1
BRAG_REPO=latent-spaces/brag
BRAG_REV=7079945d391573edebe48fdc0a23b39c4b4e8726      # v0.4.0
ADHD_REPO=ayghri/i-have-adhd
ADHD_REV=1ecea1993b7fce257ffbd686c070948b5e07eda6      # main, 2026-10-11
POCOCK_REPO=mattpocock/skills
POCOCK_REV=49dd158d1076134a641b33efb035946536778336    # main, 2026-10-09
ANTHROPIC_REPO=anthropics/skills
ANTHROPIC_REV=dbd4588f9e1033efb41dad4bef2f7947c8993d44 # main, 2026-10-09
PLAYWRIGHT=1.63.0
# modern-web-guidance is pinned by npm version and the tarball's sha512:
# its CLI (search, retrieve) ships only on npm, not in the git repository.
MWG_VERSION=1.0.0
MWG_SHA512=bda7c86700bc012e908ccdb2ca87e565b9dcee7dff4d538c2db10c45d3d3c1c172d1ff7f4eaf2ce6b250f1422a46dd8a3e5f62c6b6afc9a311e274c6b94adc9a

# fetch REPO REV TARGET... — each TARGET is "subdir:dest". One tarball
# per repository, unpacked once, and a dest is swapped in whole, so a
# failed download leaves the previous install untouched.
fetch() {
  repo="$1"; rev="$2"; shift 2
  stale=""
  for t in "$@"; do
    IFS=: read -r sub name <<< "$t"
    have=""; read -r have 2>/dev/null < "$BASE/$name/.sbxw-rev"
    [ "$have" = "$rev" ] || stale=1
  done
  if [ -z "$stale" ]; then info "$repo already at $rev"; return 0; fi
  tmp="$(mktemp -d)"
  tarball="$CACHE/tarballs/$(echo "$repo" | tr / _)-$rev.tar.gz"
  if [ -n "$CACHE" ] && [ -f "$tarball" ]; then
    tar -xzf "$tarball" -C "$tmp" || { info "the cached $repo@$rev is unreadable"; rm -rf "$tmp"; return 1; }
  elif [ -n "$FILL" ]; then
    if ! { curl -fsSL "https://codeload.github.com/$repo/tar.gz/$rev" -o "$tarball.part" && mv "$tarball.part" "$tarball" && tar -xzf "$tarball" -C "$tmp"; }; then
      info "could not download $repo@$rev"; rm -rf "$tmp" "$tarball.part"; return 1
    fi
  elif ! curl -fsSL "https://codeload.github.com/$repo/tar.gz/$rev" | tar -xz -C "$tmp"; then
    info "could not download $repo@$rev — keeping what is installed (is codeload.github.com allowed?)"
    rm -rf "$tmp"; return 1
  fi
  top="$(find "$tmp" -mindepth 1 -maxdepth 1 -type d | head -1)"
  for t in "$@"; do
    IFS=: read -r sub name <<< "$t"
    dest="$BASE/$name"
    if [ ! -f "$top/$sub/SKILL.md" ]; then
      info "$repo@$rev has no $sub/SKILL.md — skipped"; continue
    fi
    rm -rf "$dest.new" && cp -a "$top/$sub" "$dest.new" \
      && echo "$rev" > "$dest.new/.sbxw-rev" \
      && rm -rf "$dest" && mv "$dest.new" "$dest" \
      && info "$name -> $dest ($rev)"
  done
  rm -rf "$tmp"
}

# fetch_mwg — GoogleChrome/modern-web-guidance from npm: its two skills,
# modern-web-guidance (with the offline search CLI and its guides) and
# chrome-extensions. Telemetry is removed, not just switched off: the
# CLI's only network use is a `watchdog/` child process that posts usage
# (queries, guide ids, agent name) to play.googleapis.com, so that
# directory is deleted from the copy; the CLI runs fine without it (it
# also gets DISABLE_TELEMETRY=1 from its wrapper and the environment).
fetch_mwg() {
  rm -rf "$BASE/modern-web-guidance/watchdog"
  have=""; read -r have 2>/dev/null < "$BASE/modern-web-guidance/.sbxw-rev"
  have2=""; read -r have2 2>/dev/null < "$BASE/chrome-extensions/.sbxw-rev"
  if [ "$have" = "$MWG_VERSION" ] && [ "$have2" = "$MWG_VERSION" ]; then
    info "modern-web-guidance already at $MWG_VERSION"; return 0
  fi
  tmp="$(mktemp -d)"; tgz="$tmp/mwg.tgz"
  cached="$CACHE/tarballs/modern-web-guidance-$MWG_VERSION.tgz"
  if [ -n "$CACHE" ] && [ -f "$cached" ]; then
    cp "$cached" "$tgz"
  elif ! curl -fsSL "https://registry.npmjs.org/modern-web-guidance/-/modern-web-guidance-$MWG_VERSION.tgz" -o "$tgz"; then
    info "could not download modern-web-guidance@$MWG_VERSION — keeping what is installed (is registry.npmjs.org allowed?)"
    rm -rf "$tmp"; return 1
  fi
  if [ "$(sha512sum "$tgz" | cut -d' ' -f1)" != "$MWG_SHA512" ]; then
    info "modern-web-guidance@$MWG_VERSION does not match its pinned sha512 — not installed"
    rm -rf "$tmp"; return 1
  fi
  [ -n "$FILL" ] && [ ! -f "$cached" ] && cp "$tgz" "$cached.part" && mv "$cached.part" "$cached"
  tar -xzf "$tgz" -C "$tmp" || { info "modern-web-guidance@$MWG_VERSION is unreadable"; rm -rf "$tmp"; return 1; }
  rm -rf "$tmp/package/skills/modern-web-guidance/watchdog"
  for name in modern-web-guidance chrome-extensions; do
    src="$tmp/package/skills/$name"; dest="$BASE/$name"
    if [ ! -f "$src/SKILL.md" ]; then info "modern-web-guidance@$MWG_VERSION has no $name/SKILL.md — skipped"; continue; fi
    rm -rf "$dest.new" && cp -a "$src" "$dest.new" \
      && echo "$MWG_VERSION" > "$dest.new/.sbxw-rev" \
      && rm -rf "$dest" && mv "$dest.new" "$dest" \
      && info "$name -> $dest ($MWG_VERSION, telemetry removed)"
  done
  rm -rf "$tmp"
}

# The six sources are independent: fetched side by side, and
# waited for at the end. Each writes only its own dests.
fetch "$ARCHIFY_REPO" "$ARCHIFY_REV" archify:archify &
fetch "$BRAG_REPO" "$BRAG_REV" skills/brag:brag skills/brag-slim:brag-slim &
fetch "$ADHD_REPO" "$ADHD_REV" skills/i-have-adhd:i-have-adhd &
fetch "$POCOCK_REPO" "$POCOCK_REV" skills/productivity/handoff:handoff \
  skills/productivity/grill-me:grill-me skills/productivity/grilling:grilling &
fetch "$ANTHROPIC_REPO" "$ANTHROPIC_REV" skills/frontend-design:frontend-design &
fetch_mwg &

# Another kit's startup may hold apt's lock at the same moment (sbx starts
# them side by side): wait for it rather than fail on it.
APT_WAIT="-o DPkg::Lock::Timeout=120"
# apt_install ARGS... — `apt-get install` that refreshes the package
# lists only when no kit has done so in the last hour (the stamp is
# shared with the other bundled kits), and once more if the install
# fails on lists that turned out to be stale.
APT_STAMP=/home/agent/.cache/sbxw-apt-updated
apt_update() {
  if [ "$1" != force ] && [ -n "$(find "$APT_STAMP" -mmin -60 2>/dev/null)" ]; then return 0; fi
  # Lists under a week old from the cache instead of the network.
  if [ "$1" != force ] && [ -z "$FILL" ] && [ -n "$CACHE" ] && [ -n "$(find "$CACHE/apt-lists/.stamp" -mmin -10080 2>/dev/null)" ] \
      && sudo cp "$CACHE"/apt-lists/*_* /var/lib/apt/lists/ 2>/dev/null; then
    mkdir -p "$(dirname "$APT_STAMP")" && touch "$APT_STAMP"; return 0
  fi
  sudo apt-get $APT_WAIT update -qq >/dev/null 2>&1 || return 1
  mkdir -p "$(dirname "$APT_STAMP")" && touch "$APT_STAMP"
  if [ -n "$FILL" ]; then
    find /var/lib/apt/lists -maxdepth 1 -type f -name '*_*' -exec cp {} "$CACHE/apt-lists/" \; 2>/dev/null
    touch "$CACHE/apt-lists/.stamp"
  fi
}
# Packages: the filling sandbox downloads into the cache; the others
# seed apt's own archive dir from it (apt checks each file's hash
# against the lists, and downloads whatever does not match).
apt_cache_opt() {
  if [ -n "$FILL" ]; then echo "-o Dir::Cache::Archives=$CACHE/apt"; return; fi
  [ -n "$CACHE" ] && sudo cp -n "$CACHE"/apt/*.deb /var/cache/apt/archives/ 2>/dev/null
  echo ""
}
apt_install() {
  apt_update fresh || { info "apt-get update failed (are the apt mirrors in network_allow?)"; return 1; }
  opt="$(apt_cache_opt)"
  sudo DEBIAN_FRONTEND=noninteractive apt-get $APT_WAIT install $opt -y -qq --no-install-recommends "$@" >/dev/null 2>&1 && return 0
  apt_update force && sudo DEBIAN_FRONTEND=noninteractive apt-get $APT_WAIT install $opt -y -qq --no-install-recommends "$@" >/dev/null 2>&1
}

# ── Headless Chromium, for archify's browser gate and Hyperframes ────
# Playwright's build, because it exists for linux-arm64 as well as x64.
# Downloaded in the background while apt installs ffmpeg and the
# libraries Chromium links against.
find_chrome() {
  ls -d "$PW_BROWSERS"/chromium-*/chrome-linux*/chrome 2>/dev/null | sort -V | tail -1
}
CHROME="$(find_chrome)"
# The filling sandbox leaves this beside the build it downloaded. A refill
# keeps what the kits pin, so a filler that finds its own Playwright version
# stamped copies the build like the others instead of downloading it again;
# one that does not (the pin moved) empties the directory first.
PW_STAMP="$CACHE/ms-playwright/.playwright-$PLAYWRIGHT"
if [ -n "$FILL" ] && [ ! -f "$PW_STAMP" ]; then
  rm -rf "$CACHE/ms-playwright" && mkdir -p "$CACHE/ms-playwright"
fi
# Only a complete build is copied: the filler is still downloading and
# unpacking into this directory when the next sandboxes start, and half a
# Chromium copied now would be found "installed" at every later start. The
# stamp is written once the whole install is over; a cache filled before the
# stamp existed is judged on Playwright's own end-of-install marker.
cache_chrome_complete() {
  [ -n "$CACHE" ] || return 1
  [ -f "$PW_STAMP" ] && return 0
  ls "$CACHE"/ms-playwright/chromium-*/INSTALLATION_COMPLETE >/dev/null 2>&1
}
if [ -z "$CHROME" ] && cache_chrome_complete; then
  mkdir -p "$PW_BROWSERS" && cp -a "$CACHE/ms-playwright/." "$PW_BROWSERS/" && info "chromium copied from the sbxw cache"
  CHROME="$(find_chrome)"
fi
# The filling sandbox installs into the cache, then copies like the others.
PW_INSTALL="$PW_BROWSERS"; [ -n "$FILL" ] && PW_INSTALL="$CACHE/ms-playwright"
CHROME_PID=""
if [ -n "$CHROME" ]; then
  info "chromium already installed"
else
  info "installing chromium (playwright@$PLAYWRIGHT)…"
  (
    if PLAYWRIGHT_BROWSERS_PATH="$PW_INSTALL" npx -y "playwright@$PLAYWRIGHT" install chromium >/dev/null 2>&1; then
      [ -n "$FILL" ] && touch "$PW_STAMP"
    else
      info "chromium download failed (are registry.npmjs.org and cdn.playwright.dev allowed?)"
    fi
  ) &
  CHROME_PID=$!
fi

# Chromium's shared libraries, read off the binary rather than guessed
# from package names. While the binary is still downloading, the libraries it
# is known to link are looked up in the linker cache instead, so the
# install can share ffmpeg's apt call; the binary is checked again
# once it lands.
chrome_libs_missing() {
  # Matched with `case`, not `| grep -q`: under pipefail, grep quitting
  # at the first match can fail the pipeline with SIGPIPE.
  if [ -n "$CHROME" ]; then
    case "$(ldd "$CHROME" 2>/dev/null)" in *"not found"*) return 0 ;; esac
    return 1
  fi
  ldcache="$(ldconfig -p 2>/dev/null)"
  for so in libnss3.so libnspr4.so libatk-1.0.so.0 libatk-bridge-2.0.so.0 libcups.so.2 \
            libdrm.so.2 libxkbcommon.so.0 libXcomposite.so.1 libXdamage.so.1 libXfixes.so.3 \
            libXrandr.so.2 libgbm.so.1 libpango-1.0.so.0 libcairo.so.2 libasound.so.2; do
    case "$ldcache" in *"$so"*) ;; *) return 0 ;; esac
  done
  return 1
}
# Chromium's library packages on Debian 13 (sbx 0.51's image), those the
# repositories carry, in one apt-cache call (needs fresh lists: call after
# apt_update).
chrome_lib_pkgs() {
  apt-cache show libnss3 libnspr4 libatk1.0-0t64 libatk-bridge2.0-0t64 \
           libcups2t64 libdrm2 libxkbcommon0 libxcomposite1 libxdamage1 libxfixes3 \
           libxrandr2 libgbm1 libpango-1.0-0 libcairo2 libasound2t64 libxshmfence1 \
           fonts-liberation fonts-noto-color-emoji 2>/dev/null \
    | sed -n 's/^Package: //p' | sort -u | tr '\n' ' '
}

# ── FFmpeg (for /brag's poster frame and final mux) + those libraries,
#    in a single apt call ────────────────────────────────────────────
NEED_FFMPEG=""
if command -v ffmpeg >/dev/null 2>&1; then
  info "ffmpeg already installed"
else
  NEED_FFMPEG=1
fi
NEED_LIBS=""
chrome_libs_missing && NEED_LIBS=1
if [ -n "$NEED_FFMPEG$NEED_LIBS" ]; then
  PKGS=""
  [ -n "$NEED_FFMPEG" ] && info "installing ffmpeg…" && PKGS="ffmpeg"
  if [ -n "$NEED_LIBS" ] && apt_update fresh; then
    info "installing chromium's shared libraries…"
    PKGS="$PKGS $(chrome_lib_pkgs)"
  fi
  if apt_install $PKGS; then
    [ -n "$NEED_FFMPEG" ] && info "ffmpeg installed"
  else
    info "apt install failed for:$PKGS"
  fi
  command -v ffmpeg >/dev/null 2>&1 || info "ffmpeg is not installed — /brag will say so when it needs it"
fi

if [ -n "$CHROME_PID" ]; then
  wait "$CHROME_PID"
  [ -n "$FILL" ] && mkdir -p "$PW_BROWSERS" && cp -a "$CACHE/ms-playwright/." "$PW_BROWSERS/"
  CHROME="$(find_chrome)"
  # The guess above can miss: whatever the binary itself still lacks.
  if [ -n "$CHROME" ] && chrome_libs_missing; then
    apt_install $(chrome_lib_pkgs) || info "some of chromium's libraries failed to install"
  fi
fi

# `--no-sandbox` is not optional here: Chrome's own sandbox needs user
# namespaces the container does not grant to a non-root user.
# A launch costs most of a second, so it is done once per binary: the
# stamp names the binary that last started, and a restart that finds
# the same one, its wrapper in place and no library missing skips it.
CHROME_OK="$BASE/.chromium-ok"
have_ok=""; read -r have_ok 2>/dev/null < "$CHROME_OK"
if [ -n "$CHROME" ] && [ "$have_ok" = "$CHROME" ] && [ -x "$BIN/chromium" ] && ! chrome_libs_missing; then
  info "chromium already set up -> $BIN/chromium"
  HAVE_CHROME=1
elif [ -n "$CHROME" ] && "$CHROME" --headless=new --no-sandbox --disable-gpu --dump-dom about:blank >/dev/null 2>&1; then
  printf '#!/bin/sh\nexec "%s" --no-sandbox "$@"\n' "$CHROME" > "$BIN/chromium"
  chmod 0755 "$BIN/chromium"
  echo "$CHROME" > "$CHROME_OK"
  info "chromium -> $BIN/chromium"
  HAVE_CHROME=1
else
  [ -n "$CHROME" ] && info "chromium is installed but does not start — archify's browser gate will report skipped"
  # With every library there, the build itself is what is broken (a copy or
  # a download cut short): removed, so the next start fetches it again
  # instead of finding it "installed" for ever.
  if [ -n "$CHROME" ] && ! chrome_libs_missing; then
    rm -rf "$PW_BROWSERS"/chromium-* "$PW_BROWSERS"/chromium_headless_shell-*
    info "the broken chromium build is removed — the next start downloads it again"
  fi
  rm -f "$BIN/chromium" "$CHROME_OK"
  HAVE_CHROME=""
fi

# ── Environment, for every later shell the agent opens ───────────────
# Rewritten between fences, so re-running replaces rather than appends.
# The browser lines are left out when there is no browser to point at:
# archify treats a set but broken ARCHIFY_CHROME as a failure, an absent
# one as "skipped". DISABLE_TELEMETRY=1 is always there, so an agent that
# runs `npx modern-web-guidance@latest` despite its command still sends
# nothing (other CLIs that read the same variable also go quiet).
BEGIN="# >>> sbxw agent-skills >>>"
END="# <<< sbxw agent-skills <<<"
if [ -f "$ENV_FILE" ]; then
  tmpenv="$(mktemp)"
  sed "/^$BEGIN\$/,/^$END\$/d" "$ENV_FILE" > "$tmpenv"
  {
    echo "$BEGIN"
    echo "export DISABLE_TELEMETRY=1"
    if [ -n "$HAVE_CHROME" ]; then
      echo "export ARCHIFY_CHROME=$BIN/chromium"
      echo "export HYPERFRAMES_BROWSER_PATH=$BIN/chromium"
    fi
    echo "$END"
  } >> "$tmpenv"
  if ! cmp -s "$tmpenv" "$ENV_FILE"; then
    { cat "$tmpenv" > "$ENV_FILE"; } 2>/dev/null || sudo tee "$ENV_FILE" < "$tmpenv" >/dev/null \
      || info "could not update $ENV_FILE"
  fi
  rm -f "$tmpenv"
fi

wait   # the skill fetches
[ -n "$FILL" ] && touch "$CACHE/.filled-agent-skills"
info "done."
