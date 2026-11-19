#!/usr/bin/env bash
# Firecrawl Local in this sandbox: the binary and its MCP package, copied
# from sbxw's kit cache. Never built or downloaded here: building takes
# minutes and a Rust toolchain, so only the daemon's cache-fill sandbox
# (SBXW_CACHE_FILL=1) builds it, once, into the cache.
# Idempotent, and never fails the container start: whatever is missing is
# said in one line of the kit's context file and skipped.
# No dollar-brace anywhere: kit `content` fields reject it.
set -uo pipefail
info() { echo "  [firecrawl] $*"; }

# ── Startup marker ───────────────────────────────────────────────────
# Left when this script ends, however it ends, with the boot it ran in:
# how sbxw knows the install is over before it saves a template. Removed
# first, so a marker from an earlier start is never read as this one's.
SBXW_DONE="$HOME/.sbxw/startup/$(echo "$0" | tr / _).done"
mkdir -p "$HOME/.sbxw/startup" && rm -f "$SBXW_DONE"
trap 'cat /proc/sys/kernel/random/boot_id > "$SBXW_DONE" 2>/dev/null' EXIT

# ── Pins: the one place to bump. Move FIRECRAWL_VERSION and its crate
#    sha256 together (crates.io's "checksum" for that version); the MCP
#    package is pinned by mcp/package-lock.json beside this script, every
#    package with its integrity hash. A cache holding the previous pins
#    keeps serving them until the next fill (`sbxw cache clear` forces one).
FIRECRAWL_VERSION=0.1.0
FIRECRAWL_CRATE_SHA256=695214611dce0881c25c5677ec70674c330e1f7a81297eb50b91273086e8e79c
MCP_VERSION=3.40.0
# The fill's toolchain: firecrawl-local 0.1.0's lock needs Rust 1.89 or
# newer, and Debian 13 ships 1.85. rustup-init is pinned by version and
# per-architecture sha256 (static.rust-lang.org/rustup/archive); rustup
# checks the toolchain it downloads against the channel manifest.
RUST_TOOLCHAIN=1.99.1
RUSTUP_VERSION=1.29.3

KIT=$HOME/.sbxw/firecrawl
LIB="$KIT/lib"                       # installed: <version>/firecrawl-local, mcp-<version>/
CONTEXT=$HOME/.claude/kits-agent-context/firecrawl.md
mkdir -p "$LIB"

case "$(uname -m)" in
  x86_64)        RUST_TARGET=x86_64-unknown-linux-gnu
                 RUSTUP_SHA256=b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55 ;;
  aarch64|arm64) RUST_TARGET=aarch64-unknown-linux-gnu
                 RUSTUP_SHA256=9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282 ;;
  *)             RUST_TARGET=""; RUSTUP_SHA256="" ;;
esac

# ── The sbxw host cache ──────────────────────────────────────────────
# Mounted read-only in every sandbox someone works in, writable only in
# the daemon's filler (SBXW_CACHE_FILL=1). printenv, not the variable:
# set -u, and no dollar-brace defaults in kit content.
CACHE=""; FILL=""
SBXW_CACHE="$(printenv SBXW_CACHE)"
if [ -n "$SBXW_CACHE" ] && [ -d "$SBXW_CACHE" ]; then
  CACHE="$SBXW_CACHE/$(dpkg --print-architecture 2>/dev/null)-$(. /etc/os-release 2>/dev/null; echo "$VERSION_CODENAME")"
  if [ "$(printenv SBXW_CACHE_FILL)" = 1 ]; then
    FILL=1; mkdir -p "$CACHE/firecrawl"
  fi
  [ -d "$CACHE" ] || CACHE=""
fi
STORE="$CACHE/firecrawl"

sha256_of() { sha256sum "$1" 2>/dev/null | cut -d' ' -f1; }

# ── The fill: build the pinned binary and install the pinned MCP package
#    into the cache. Only in the daemon's filler. ─────────────────────
fill_binary() {
  out="$STORE/$FIRECRAWL_VERSION"
  if [ -f "$out/.complete" ]; then info "cache: firecrawl-local $FIRECRAWL_VERSION already built"; return 0; fi
  [ -n "$RUST_TARGET" ] || { info "cache: no Rust toolchain for $(uname -m) — not built"; return 1; }
  work="$(mktemp -d)" || return 1
  # A C compiler for the crates' build scripts.
  if ! command -v cc >/dev/null 2>&1; then
    info "cache: installing build-essential…"
    sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=120 update -qq >/dev/null 2>&1
    sudo DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=120 install -y -qq --no-install-recommends build-essential >/dev/null 2>&1 \
      || { info "cache: build-essential did not install (are the Debian mirrors allowed?)"; rm -rf "$work"; return 1; }
  fi
  # The toolchain lives in the filler's temporary directory and goes with it.
  export RUSTUP_HOME="$work/rustup" CARGO_HOME="$work/cargo"
  if ! curl -fsSL "https://static.rust-lang.org/rustup/archive/$RUSTUP_VERSION/$RUST_TARGET/rustup-init" -o "$work/rustup-init"; then
    info "cache: rustup-init did not download (is static.rust-lang.org allowed?)"; rm -rf "$work"; return 1
  fi
  if [ "$(sha256_of "$work/rustup-init")" != "$RUSTUP_SHA256" ]; then
    info "cache: rustup-init does not match its pinned sha256 — not built"; rm -rf "$work"; return 1
  fi
  chmod +x "$work/rustup-init"
  "$work/rustup-init" -y -q --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN" >/dev/null 2>&1 \
    || { info "cache: Rust $RUST_TOOLCHAIN did not install (is static.rust-lang.org allowed?)"; rm -rf "$work"; return 1; }
  # The crate itself, checked against its pinned sha256, then built with
  # the lock it was published with (every dependency checksummed there).
  if ! curl -fsSL "https://static.crates.io/crates/firecrawl-local/firecrawl-local-$FIRECRAWL_VERSION.crate" -o "$work/crate.tgz"; then
    info "cache: the crate did not download (is static.crates.io allowed?)"; rm -rf "$work"; return 1
  fi
  if [ "$(sha256_of "$work/crate.tgz")" != "$FIRECRAWL_CRATE_SHA256" ]; then
    info "cache: firecrawl-local-$FIRECRAWL_VERSION.crate does not match its pinned sha256 — not built"; rm -rf "$work"; return 1
  fi
  mkdir -p "$work/src" && tar -xzf "$work/crate.tgz" -C "$work/src" || { info "cache: the crate is unreadable"; rm -rf "$work"; return 1; }
  # Its own workspace, whatever directory the temporary one sits under.
  printf '\n[workspace]\n' >> "$work/src/firecrawl-local-$FIRECRAWL_VERSION/Cargo.toml"
  info "cache: building firecrawl-local $FIRECRAWL_VERSION with Rust $RUST_TOOLCHAIN (minutes)…"
  started="$(date +%s)"
  if ! "$CARGO_HOME/bin/cargo" install --locked --path "$work/src/firecrawl-local-$FIRECRAWL_VERSION" --root "$work/out" > "$work/build.log" 2>&1; then
    info "cache: the build failed (are index.crates.io and static.crates.io allowed?); last lines:"
    tail -5 "$work/build.log" | sed 's/^/    /'
    rm -rf "$work"; return 1
  fi
  bin="$work/out/bin/firecrawl-local"
  rm -rf "$out.part" && mkdir -p "$out.part" && cp "$bin" "$out.part/firecrawl-local" \
    && sha256_of "$out.part/firecrawl-local" > "$out.part/firecrawl-local.sha256" \
    && echo "firecrawl-local $FIRECRAWL_VERSION, crate sha256 $FIRECRAWL_CRATE_SHA256, Rust $RUST_TOOLCHAIN, $RUST_TARGET, built in $(( $(date +%s) - started ))s on $(date -u +%FT%TZ)" > "$out.part/BUILD" \
    && touch "$out.part/.complete" && rm -rf "$out" && mv "$out.part" "$out" \
    && info "cache: firecrawl-local $FIRECRAWL_VERSION built ($(cut -c1-12 "$out/firecrawl-local.sha256"))"
  rm -rf "$work"
  [ -f "$out/.complete" ]
}

fill_mcp() {
  out="$STORE/mcp-$MCP_VERSION"
  if [ -f "$out/.complete" ]; then info "cache: firecrawl-mcp $MCP_VERSION already there"; return 0; fi
  rm -rf "$out.part" && mkdir -p "$out.part" && cp "$KIT/mcp/package.json" "$KIT/mcp/package-lock.json" "$out.part/" || return 1
  # npm ci installs exactly the lock, checking every integrity hash, and
  # runs no package script.
  if ! (cd "$out.part" && npm ci --ignore-scripts --omit=dev --no-audit --no-fund --loglevel=error >/dev/null 2>&1); then
    info "cache: firecrawl-mcp $MCP_VERSION did not install (is registry.npmjs.org allowed?)"; rm -rf "$out.part"; return 1
  fi
  [ -f "$out.part/node_modules/firecrawl-mcp/dist/index.js" ] || { info "cache: firecrawl-mcp $MCP_VERSION has no dist/index.js"; rm -rf "$out.part"; return 1; }
  touch "$out.part/.complete" && rm -rf "$out" && mv "$out.part" "$out" && info "cache: firecrawl-mcp $MCP_VERSION installed"
}

if [ -n "$FILL" ]; then
  fill_binary; b=$?
  fill_mcp; m=$?
  # Marked only when both are there: an unmarked cache is filled again.
  [ "$b" = 0 ] && [ "$m" = 0 ] && touch "$CACHE/.filled-firecrawl"
fi

# ── Install from the cache ───────────────────────────────────────────
# The pinned version when the cache has it, else what is installed, else
# the newest complete one the cache holds (a cache filled before the pins
# moved). A copy is checked against the hash the filler recorded beside it.
newest_in() { ls -d "$1"/$2 2>/dev/null | while read -r d; do [ -f "$d/.complete" ] && basename "$d"; done | sort -V | tail -1; }

install_binary() {
  want="$FIRECRAWL_VERSION"; have=""; read -r have 2>/dev/null < "$LIB/binary.version"
  if [ "$have" = "$want" ] && [ -x "$LIB/$have/firecrawl-local" ]; then info "firecrawl-local $have already installed"; return 0; fi
  if [ -z "$CACHE" ] || [ ! -f "$STORE/$want/.complete" ]; then
    if [ -n "$have" ] && [ -x "$LIB/$have/firecrawl-local" ]; then info "firecrawl-local $have kept ($want is not in the cache yet)"; return 0; fi
    want="$( [ -n "$CACHE" ] && newest_in "$STORE" '[0-9]*')"
    [ -n "$want" ] || return 1
  fi
  src="$STORE/$want"
  rm -rf "$LIB/$want.part" && mkdir -p "$LIB/$want.part" && cp "$src/firecrawl-local" "$LIB/$want.part/" || { rm -rf "$LIB/$want.part"; return 1; }
  sum=""; read -r sum 2>/dev/null < "$src/firecrawl-local.sha256"
  if [ -z "$sum" ] || [ "$(sha256_of "$LIB/$want.part/firecrawl-local")" != "$sum" ]; then
    info "the cached firecrawl-local $want does not match its recorded sha256 — not installed"; rm -rf "$LIB/$want.part"; return 1
  fi
  chmod 0755 "$LIB/$want.part/firecrawl-local" && rm -rf "$LIB/$want" && mv "$LIB/$want.part" "$LIB/$want" \
    && echo "$want" > "$LIB/binary.version" && info "firecrawl-local $want installed from the sbxw cache"
  # Earlier versions go once the new one is in place.
  for d in "$LIB"/[0-9]*; do [ -d "$d" ] && [ "$(basename "$d")" != "$want" ] && rm -rf "$d"; done
  return 0
}

install_mcp() {
  want="mcp-$MCP_VERSION"; have=""; read -r have 2>/dev/null < "$LIB/mcp.version"
  if [ "$have" = "$want" ] && [ -f "$LIB/$have/node_modules/firecrawl-mcp/dist/index.js" ]; then info "firecrawl-mcp $MCP_VERSION already installed"; return 0; fi
  if [ -z "$CACHE" ] || [ ! -f "$STORE/$want/.complete" ]; then
    if [ -n "$have" ] && [ -f "$LIB/$have/node_modules/firecrawl-mcp/dist/index.js" ]; then info "$have kept ($want is not in the cache yet)"; return 0; fi
    want="$( [ -n "$CACHE" ] && newest_in "$STORE" 'mcp-*')"
    [ -n "$want" ] || return 1
  fi
  rm -rf "$LIB/$want.part" && cp -a "$STORE/$want" "$LIB/$want.part" \
    && [ -f "$LIB/$want.part/node_modules/firecrawl-mcp/dist/index.js" ] \
    && rm -rf "$LIB/$want" && mv "$LIB/$want.part" "$LIB/$want" \
    && echo "$want" > "$LIB/mcp.version" && info "firecrawl-mcp ($want) installed from the sbxw cache" \
    || { rm -rf "$LIB/$want.part"; info "the cached $want is unreadable — not installed"; return 1; }
  for d in "$LIB"/mcp-*; do [ -d "$d" ] && [ "$(basename "$d")" != "$want" ] && rm -rf "$d"; done
  return 0
}

install_binary; have_bin=$?
install_mcp; have_mcp=$?

# ── The note in the kit's context file ───────────────────────────────
# sbx writes ~/.claude/kits-agent-context/firecrawl.md from the kit's
# context; the note goes between its own fences at the end, rewritten at
# every start, and is gone once Firecrawl is there.
BEGIN="<!-- sbxw:firecrawl-note:begin -->"
END="<!-- sbxw:firecrawl-note:end -->"
note=""
if [ "$have_bin" != 0 ]; then
  if [ -n "$CACHE" ]; then
    note="Firecrawl is not installed yet: sbxw is preparing it; use WebFetch meanwhile."
  else
    note="Firecrawl is not installed: this sandbox has no sbxw kit cache to copy it from; use WebFetch."
  fi
elif [ "$have_mcp" != 0 ]; then
  note="Firecrawl's MCP server is not installed yet (sbxw is preparing it): use the firecrawl-local command, or WebFetch."
fi
[ -n "$note" ] && info "$note"
if [ -f "$CONTEXT" ] || [ -n "$note" ]; then
  mkdir -p "$(dirname "$CONTEXT")"
  cur=""; [ -f "$CONTEXT" ] && cur="$(sed "/^$BEGIN\$/,/^$END\$/d" "$CONTEXT")"
  new="$cur"
  [ -n "$note" ] && new="$(printf '%s\n\n%s\n%s\n%s' "$cur" "$BEGIN" "$note" "$END")"
  if [ "$new" != "$(cat "$CONTEXT" 2>/dev/null)" ]; then
    printf '%s\n' "$new" > "$CONTEXT.sbxw-tmp" && mv "$CONTEXT.sbxw-tmp" "$CONTEXT" || rm -f "$CONTEXT.sbxw-tmp"
  fi
fi
info "done."
exit 0
