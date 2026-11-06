#!/bin/sh
# Node.js for sbxw's own tooling: the relay MCP server, the session hooks, the
# status line and the bundled kits' .mjs scripts all run on `node`, and sbx
# 0.51's Claude Code image (Docker Hardened Images, Debian 13) ships none.
#
# Runs once per sandbox, as root, before the agent starts (a lifecycle install
# hook). Never fails the create: sbxw reports a missing node itself, naming
# what does not work without it.
set -u
export DEBIAN_FRONTEND=noninteractive

have() { command -v "$1" >/dev/null 2>&1; }
say() { printf 'sbxw-base: %s\n' "$*" >&2; }

if have node && have npm; then
  say "node $(node --version) already installed"
  exit 0
fi

apt-get update -qq >/dev/null 2>&1 || say "apt-get update failed; trying the package lists the image has"

# The image's DHI repository carries versioned toolchains (nodejs-24); a
# plain Debian image has trixie's nodejs. Whichever is there first.
for pkg in nodejs-24 nodejs; do
  if apt-get install -y -qq --no-install-recommends "$pkg" >/dev/null 2>&1; then
    say "installed $pkg"
    # A versioned package may keep its binaries out of PATH.
    if ! have node; then
      bin=$(dpkg -L "$pkg" 2>/dev/null | grep -E '/bin/node$' | head -n 1)
      if [ -n "$bin" ]; then
        dir=$(dirname "$bin")
        for tool in node npm npx corepack; do
          [ -x "$dir/$tool" ] && ln -sf "$dir/$tool" "/usr/local/bin/$tool"
        done
      fi
    fi
    break
  fi
done

# npm/npx: the agent-skills kit installs Playwright with npx.
have npm || apt-get install -y -qq --no-install-recommends npm >/dev/null 2>&1 || true

if have node; then
  say "node $(node --version), npm $(npm --version 2>/dev/null || echo missing)"
else
  say "could not install Node.js — sbxw's hooks, relay and status line will not run in this sandbox"
fi
exit 0
