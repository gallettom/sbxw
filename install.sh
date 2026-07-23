#!/usr/bin/env sh
# sbxw installer
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/gallettom/sbxw/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/gallettom/sbxw/main/install.sh | sh -s v1.0.0
set -eu

REPO="gallettom/sbxw"
BINARY="sbxw"
INSTALL_DIR="${SBXW_INSTALL_DIR:-/usr/local/bin}"
KITS_DIR="${SBXW_KITS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/sbxw/kits}"

# ── Colours (disabled when not a tty) ────────────────────────────────────────
if [ -t 1 ]; then
  RED='\033[0;31m'; YELLOW='\033[1;33m'; GREEN='\033[0;32m'
  BOLD='\033[1m'; RESET='\033[0m'
else
  RED=''; YELLOW=''; GREEN=''; BOLD=''; RESET=''
fi

info()  { printf "${GREEN}✓${RESET} %s\n" "$*"; }
warn()  { printf "${YELLOW}⚠${RESET}  %s\n" "$*" >&2; }
err()   { printf "${RED}✗${RESET}  %s\n" "$*" >&2; }
step()  { printf "\n${BOLD}%s${RESET}\n" "$*"; }
die()   { err "$*"; exit 1; }

# Adds a single `source <(sbxw completion <shell>)` line (or fish's pipe
# equivalent) to the shell's rc file, once. Regenerated fresh on every new
# shell from whatever sbxw is currently installed, so — unlike a completion
# script written to disk — it never goes stale after `sbxw update`.
install_completions() {
  sh_name="$1"
  case "$sh_name" in
    zsh)  rc="$HOME/.zshrc";  line="source <(sbxw completion zsh)" ;;
    bash) rc="$HOME/.bashrc"; line="source <(sbxw completion bash)" ;;
    fish) rc="$HOME/.config/fish/config.fish"; line="sbxw completion fish | source" ;;
    *) return 0 ;;
  esac

  mkdir -p "$(dirname "$rc")"
  if [ -f "$rc" ] && grep -qF "sbxw completion" "$rc" 2>/dev/null; then
    info "already configured in ${rc}"
  else
    printf '\n# sbxw shell completion\n%s\n' "$line" >> "$rc"
    info "added to ${rc}"
  fi
  printf "  Open a new shell (or: source %s) to activate it.\n" "$rc"
}

# Download the prebuilt macOS menu-bar app (sbxw Island) and install it into
# ~/Applications. The bundle is ad-hoc-signed (not notarised), so we strip the
# download quarantine to let Gatekeeper run it.
install_island() {
  url="$1"; app_dir="$2"
  tmp="$(mktemp -d)"
  if ! curl -fsSL "$url" -o "$tmp/island.zip" 2>/dev/null; then
    warn "No island build in ${VERSION} (skipping)."
    rm -rf "$tmp"; return 0
  fi
  if command -v ditto >/dev/null 2>&1; then
    ditto -x -k "$tmp/island.zip" "$tmp" 2>/dev/null || true
  else
    unzip -q "$tmp/island.zip" -d "$tmp" 2>/dev/null || true
  fi
  if [ ! -d "$tmp/SbxwIsland.app" ]; then
    warn "Island archive missing SbxwIsland.app (skipping)."
    rm -rf "$tmp"; return 0
  fi
  mkdir -p "$app_dir"
  rm -rf "$app_dir/SbxwIsland.app"
  mv "$tmp/SbxwIsland.app" "$app_dir/"
  xattr -dr com.apple.quarantine "$app_dir/SbxwIsland.app" 2>/dev/null || true
  rm -rf "$tmp"
  info "island installed → ${app_dir}/SbxwIsland.app"
  printf "  Launch it:  open '%s/SbxwIsland.app'\n" "$app_dir"
  printf "  On first click-to-focus it asks permission to control your browser — allow it.\n"
  printf "  Want it at startup? Add it under System Settings › General › Login Items.\n"
}

# ── Check sbx dependency ─────────────────────────────────────────────────────
step "Checking prerequisites…"

if ! command -v sbx >/dev/null 2>&1; then
  err "'sbx' (Docker Sandboxes CLI) is not on your PATH."
  printf "\n  sbxw is a wrapper around 'sbx' and cannot work without it.\n"
  printf "  Install Docker Desktop (which bundles sbx), then re-run this script:\n"
  printf "\n    https://docs.docker.com/get-started/get-docker/\n\n"
  printf "  Once Docker Desktop is installed, verify with:\n"
  printf "    sbx version\n\n"
  exit 1
fi

SBX_VERSION="$(sbx version 2>/dev/null | head -1 || true)"
info "sbx found${SBX_VERSION:+ — $SBX_VERSION}"

# Check that the user has logged in (sbx login sets up credentials).
# `sbx ls` is a lightweight read-only call; failure usually means not logged in.
if ! sbx ls >/dev/null 2>&1; then
  warn "'sbx ls' failed — you may need to run 'sbx login' first."
  printf "  Run:  sbx login\n"
  printf "  Then re-run this installer (or just keep going — sbxw will remind you).\n"
fi

# ── Detect OS ─────────────────────────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
  Darwin) OS="macos" ;;
  Linux)  OS="linux" ;;
  *) die "Unsupported OS: $OS" ;;
esac

# ── Detect architecture ───────────────────────────────────────────────────────
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)         ARCH="x86_64" ;;
  arm64|aarch64)  ARCH="arm64" ;;
  *) die "Unsupported architecture: $ARCH" ;;
esac

# ── Resolve version ───────────────────────────────────────────────────────────
step "Resolving version…"
VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
fi
[ -n "$VERSION" ] || die "Could not determine latest version. Pass a version as the first argument, e.g.: sh -s v1.0.0"

ARTIFACT="${BINARY}-${OS}-${ARCH}"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"

step "Downloading sbxw ${VERSION} (${OS}/${ARCH})…"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

curl -fsSL --progress-bar "$URL" -o "$TMP" || die "Download failed. Check that ${VERSION} has a release at https://github.com/${REPO}/releases"
chmod +x "$TMP"

# ── Install binary ─────────────────────────────────────────────────────────────
step "Installing binary…"
if [ -w "$INSTALL_DIR" ]; then
  mv "$TMP" "${INSTALL_DIR}/${BINARY}"
else
  printf "  sudo required to write to %s\n" "$INSTALL_DIR"
  sudo mv "$TMP" "${INSTALL_DIR}/${BINARY}"
fi

info "sbxw installed → ${INSTALL_DIR}/${BINARY}"
"${INSTALL_DIR}/${BINARY}" --version

# ── PATH check ────────────────────────────────────────────────────────────────
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    warn "${INSTALL_DIR} is not in your PATH."
    printf "  Add this line to your shell profile (~/.zshrc, ~/.bashrc, etc.):\n"
    printf "\n    export PATH=\"%s:\$PATH\"\n\n" "$INSTALL_DIR"
    ;;
esac

# ── Shell completions (optional) ────────────────────────────────────────────
step "Shell completions…"
SHELL_NAME="$(basename "${SHELL:-}" 2>/dev/null || true)"
# `-r /dev/tty` only checks permission bits — the node can exist yet have no
# controlling terminal behind it (e.g. CI, `curl | sh` with stdin redirected
# from a file). Actually try to open it; that's the real test.
if ! (: 2>/dev/null >/dev/tty) 2>/dev/null; then
  warn "Non-interactive install — skipping the completions prompt."
  printf "  Run 'sbxw completion --help' any time to set them up.\n"
elif [ "$SHELL_NAME" != "bash" ] && [ "$SHELL_NAME" != "zsh" ] && [ "$SHELL_NAME" != "fish" ]; then
  warn "Could not detect a supported shell (\$SHELL=${SHELL:-unset})."
  printf "  Run 'sbxw completion --help' to set them up manually (bash/zsh/fish/elvish/powershell).\n"
else
  printf "Enable TAB-completion for sbxw in %s? [Y/n] " "$SHELL_NAME" > /dev/tty
  read -r REPLY < /dev/tty || REPLY=""
  case "$REPLY" in
    [nN]*)
      info "Skipped. Run 'sbxw completion --help' any time to install it later."
      ;;
    *)
      install_completions "$SHELL_NAME"
      ;;
  esac
fi

# ── Bundled kits (optional) ───────────────────────────────────────────────────
step "Fetching bundled kits…"
KITS_URL="https://github.com/${REPO}/releases/download/${VERSION}/sbxw-kits.tar.gz"
KITS_TMP="$(mktemp)"
if curl -fsSL "$KITS_URL" -o "$KITS_TMP" 2>/dev/null; then
  mkdir -p "$KITS_DIR"
  tar -xzf "$KITS_TMP" -C "$KITS_DIR"
  info "kits installed → ${KITS_DIR}"
else
  warn "No kits tarball in this release (skipping)."
fi
rm -f "$KITS_TMP"

# ── macOS menu-bar island (optional) ─────────────────────────────────────────
if [ "$OS" = "macos" ]; then
  step "Menu-bar island (optional)…"
  ISLAND_URL="https://github.com/${REPO}/releases/download/${VERSION}/SbxwIsland-macos.zip"
  ISLAND_APP_DIR="$HOME/Applications"
  if ! (: 2>/dev/null >/dev/tty) 2>/dev/null; then
    warn "Non-interactive install — skipping the island."
    printf "  Get it later from https://github.com/%s/releases (SbxwIsland-macos.zip).\n" "$REPO"
  else
    printf "Install the sbxw Island menu-bar app (live session state in the notch)? [y/N] " > /dev/tty
    read -r REPLY < /dev/tty || REPLY=""
    case "$REPLY" in
      [yY]*) install_island "$ISLAND_URL" "$ISLAND_APP_DIR" ;;
      *)     info "Skipped. Grab it any time from the releases page." ;;
    esac
  fi
fi

# ── Detect auth ───────────────────────────────────────────────────────────────
HAS_API_KEY=0
HAS_OAUTH=0
[ -n "${ANTHROPIC_API_KEY:-}" ]         && HAS_API_KEY=1
[ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]   && HAS_OAUTH=1
[ -n "${CLAUDE_OAUTH_TOKEN:-}" ]        && HAS_OAUTH=1

# ── Next steps ────────────────────────────────────────────────────────────────
printf "\n${BOLD}══════════════════════════════════════════${RESET}\n"
printf "${BOLD} sbxw %s ready!${RESET}\n" "$VERSION"
printf "${BOLD}══════════════════════════════════════════${RESET}\n\n"

printf "${BOLD}1. Auth (pick one)${RESET}\n"
if [ "$HAS_API_KEY" -eq 1 ]; then
  printf "   ${GREEN}✓${RESET} ANTHROPIC_API_KEY detected — use: sbxw up <name> --use-api-key\n"
elif [ "$HAS_OAUTH" -eq 1 ]; then
  printf "   ${GREEN}✓${RESET} CLAUDE_CODE_OAUTH_TOKEN detected — sbxw will inject it automatically.\n"
else
  printf "   ${YELLOW}⚠${RESET}  No auth token found in environment. Choose one:\n"
  printf "   a) API key:    export ANTHROPIC_API_KEY=sk-ant-…\n"
  printf "                  sbxw up <name> --use-api-key\n"
  printf "   b) OAuth:      export CLAUDE_CODE_OAUTH_TOKEN=\$(claude setup-token)\n"
  printf "   c) Interactive: run /login inside the web terminal after startup.\n"
fi

printf "\n${BOLD}2. Project config${RESET}\n"
printf "   Copy the example config into your project root:\n"
if [ -f "${KITS_DIR}/../sbxw.toml.example" ] 2>/dev/null; then
  printf "     cp %s/sbxw.toml.example ./sbxw.toml\n" "$(dirname "$KITS_DIR")"
else
  printf "     curl -fsSL https://raw.githubusercontent.com/%s/main/sbxw.toml.example -o sbxw.toml\n" "$REPO"
fi
printf "   Edit the [[ports]] and aliases to match your project.\n"

printf "\n${BOLD}3. Launch${RESET}\n"
printf "   cd /path/to/your/project\n"
printf "   sbxw up <sandbox-name>        # provisions + opens browser terminal\n"
printf "   sbxw up                       # web UI only (create sandboxes from browser)\n"
printf "   sbxw --help                   # all commands\n"

printf "\n${BOLD}Useful extras${RESET}\n"
printf "   sbxw bash <name>              # interactive shell in the sandbox\n"
printf "   sbxw logs <name>              # tail daemon log\n"
printf "   sbxw down                     # kill all daemons + clean /etc/hosts\n"
printf "   sbxw update                   # update sbxw itself to the latest release\n"
printf "   sbxw completion --help       # TAB-complete commands (bash/zsh/fish/…)\n"
if [ -d "$KITS_DIR" ]; then
  printf "   kits: %s/\n" "$KITS_DIR"
  printf "         reference one in sbxw.toml, e.g.:\n"
  printf "         kits = [\"%s/k8s-tools\"]\n" "$KITS_DIR"
fi
printf "\n"
