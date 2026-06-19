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
if [ -d "$KITS_DIR" ]; then
  printf "   kits: %s/\n" "$KITS_DIR"
  printf "         reference one in sbxw.toml, e.g.:\n"
  printf "         kits = [\"%s/k8s-tools\"]\n" "$KITS_DIR"
fi
printf "\n"
