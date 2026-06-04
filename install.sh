#!/usr/bin/env sh
# sbxw installer
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/YOUR_ORG/sbxw/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/YOUR_ORG/sbxw/main/install.sh | sh -s v0.2.0
set -eu

REPO="gallettom/sbxw"
BINARY="sbxw"
INSTALL_DIR="${SBXW_INSTALL_DIR:-/usr/local/bin}"
# Bundled kits (e.g. k8s-tools) are shipped as a separate tarball and extracted
# here so you can reference them from sbxw.toml after a binary-only install.
KITS_DIR="${SBXW_KITS_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/sbxw/kits}"

# ── Detect OS ────────────────────────────────────────────────────────────────
OS="$(uname -s)"
case "$OS" in
  Darwin) OS="macos" ;;
  Linux)  OS="linux" ;;
  *) echo "Unsupported OS: $OS" >&2; exit 1 ;;
esac

# ── Detect architecture ──────────────────────────────────────────────────────
ARCH="$(uname -m)"
case "$ARCH" in
  x86_64)         ARCH="x86_64" ;;
  arm64|aarch64)  ARCH="arm64" ;;
  *) echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

# ── Resolve version ──────────────────────────────────────────────────────────
VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)"
fi
if [ -z "$VERSION" ]; then
  echo "Could not determine latest version. Pass a version as the first argument." >&2
  exit 1
fi

ARTIFACT="${BINARY}-${OS}-${ARCH}"
URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"

echo "Installing sbxw ${VERSION} (${OS}/${ARCH}) …"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

curl -fsSL --progress-bar "$URL" -o "$TMP"
chmod +x "$TMP"

# ── Install ───────────────────────────────────────────────────────────────────
if [ -w "$INSTALL_DIR" ]; then
  mv "$TMP" "${INSTALL_DIR}/${BINARY}"
else
  echo "→ sudo required to write to ${INSTALL_DIR}"
  sudo mv "$TMP" "${INSTALL_DIR}/${BINARY}"
fi

echo "✓ sbxw installed → ${INSTALL_DIR}/${BINARY}"
"${INSTALL_DIR}/${BINARY}" --version

# ── Bundled kits (optional) ─────────────────────────────────────────────────
# Fetch the kits tarball from the same release if present. A binary-only
# release (no kits asset) degrades gracefully — install still succeeds.
KITS_URL="https://github.com/${REPO}/releases/download/${VERSION}/sbxw-kits.tar.gz"
KITS_TMP="$(mktemp)"
if curl -fsSL "$KITS_URL" -o "$KITS_TMP" 2>/dev/null; then
  mkdir -p "$KITS_DIR"
  tar -xzf "$KITS_TMP" -C "$KITS_DIR"
  echo "✓ kits installed → ${KITS_DIR}"
  echo "  reference one in sbxw.toml, e.g.:"
  echo "    kits = [\"${KITS_DIR}/k8s-tools\"]"
fi
rm -f "$KITS_TMP"

cat <<EOF

Next steps:
  sbxw up                 # start the web daemon → http://sbxw.localhost:7681
  sbxw up <name> [path]   # provision + attach a sandbox (path defaults to cwd)
  sbxw bash <name>        # open a bash shell in a sandbox
  sbxw --help             # all commands
EOF
