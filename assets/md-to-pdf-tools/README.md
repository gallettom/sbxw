# md-to-pdf-tools kit

Ships the `/md-to-pdf` skill itself — written to
`/home/agent/.claude/commands/md-to-pdf.md` (user-level, so it's available
regardless of which project is mounted as the workspace) — and pre-installs
the dependencies it needs: **WeasyPrint**, **poppler-utils** (`pdftoppm`),
**Pillow**, and **python-markdown**. Together this means Markdown → PDF/PNG
conversion is available and works on first invocation, with no manual copy
step and no install step. Idempotent — re-running skips anything already
installed, and rewrites the skill file to whatever version ships with this
kit.

## Usage

Reference the kit *directory* (not a single file) from your `sbxw.toml`:

```toml
kits = [
  "path/to/sbxw/assets/md-to-pdf-tools",
]
```

sbxw applies it via `sbx kit add` on every `sbxw up`, **after** the network
policy so the installers have egress access.

## Required network allowlist

The apt step will `403` unless the Debian/Ubuntu mirrors are in your
`sbxw.toml` `network_allow` (sbxw applies the policy before the kit runs):

```toml
network_allow = [
  # ...defaults... (pypi.org / *.pythonhosted.org are already default)
  "deb.debian.org", "security.debian.org",
  "archive.ubuntu.com", "security.ubuntu.com", "ports.ubuntu.com",
]
```

> The kit's own `network.allowedDomains` block is metadata; the effective
> egress policy comes from sbxw's `network_allow`. Keep the two in sync.

## Manual apply (existing sandbox)

```sh
sbx kit add <sandbox> path/to/sbxw/assets/md-to-pdf-tools
sbx kit validate path/to/sbxw/assets/md-to-pdf-tools   # sanity check the spec
```

## Notes

- Without this kit, `/md-to-pdf` is only available in a sandbox if the target
  project happens to carry its own `.claude/commands/md-to-pdf.md`. The kit's
  dependency checks are self-healing either way — even the copy it ships
  installs whatever's missing on first run — so this kit's only real job is
  moving both the skill file and the install cost to `sbxw up` time, instead
  of requiring a manual copy and a slow first conversion.
- `spec.yaml` `content` fields do **not** allow `${VAR}` placeholders (only
  `${WORKDIR}`). The install script uses brace-free `$VAR` syntax for that reason.
- `startup` commands are exec-style arrays (`command: ["bash", "..."]`), not
  shell strings.
