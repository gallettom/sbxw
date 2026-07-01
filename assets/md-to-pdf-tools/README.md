# md-to-pdf-tools kit

Pre-installs the dependencies used by the `/md-to-pdf` skill — **WeasyPrint**,
**poppler-utils** (`pdftoppm`), **Pillow**, and **python-markdown** — so
Markdown → PDF/PNG conversion works on first invocation with no install step.
Idempotent — re-running skips anything already installed.

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

- Without this kit, `/md-to-pdf` still works — it checks for each dependency
  and installs whatever's missing on first run. This kit just moves that cost
  to `sbxw up` time so the first conversion isn't the slow one.
- `spec.yaml` `content` fields do **not** allow `${VAR}` placeholders (only
  `${WORKDIR}`). The install script uses brace-free `$VAR` syntax for that reason.
- `startup` commands are exec-style arrays (`command: ["bash", "..."]`), not
  shell strings.
