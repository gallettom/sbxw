# k8s-tools kit

Installs **kubectl**, **k3d**, and **skaffold** into `~/.local/bin` inside a
sandbox. Architecture-aware (amd64 / arm64) and idempotent — re-running skips
anything already installed. The three downloads run side by side, each into a
temp file moved into place only when complete; a failed one is reported in the
start log and skipped, never failing the container start.

## Usage

Reference the kit *directory* (not a single file) from your `sbxw.toml`:

```toml
kits = [
  "path/to/sbxw/assets/k8s-tools",
]
```

sbxw applies it via `sbx kit add` on every `sbxw up`, **after** the network
policy so the installers have egress access.

## Required network allowlist

The installers will `403` unless these domains are in your `sbxw.toml`
`network_allow` (sbxw applies the policy before the kit runs):

```toml
network_allow = [
  # ...defaults...
  "dl.k8s.io", "cdn.dl.k8s.io",                  # kubectl (dl.k8s.io 302→cdn)
  "get.k3d.io", "raw.githubusercontent.com",
  "objects.githubusercontent.com",               # k3d install.sh + release binary
  "storage.googleapis.com",                      # skaffold
]
```

> The kit's `network-policy@1` allow rules join the sandbox's policy when it is
> applied, so sbxw.toml needs no `network_allow` entry for them.

## Manual apply (existing sandbox)

```sh
sbx kit add <sandbox> path/to/sbxw/assets/k8s-tools
```

## Notes

- The kit's files live under `files/home/agent/` and reach the sandbox as image
  content (kit spec v3 `build:`), byte for byte: nothing in them is expanded.
- `startup` commands are exec-style arrays (`command: ["bash", "..."]`), not
  shell strings.
- Running a k3d cluster also needs Docker-in-Docker with privileged containers
  and image pulls from Docker Hub / quay.io — allow those separately.
