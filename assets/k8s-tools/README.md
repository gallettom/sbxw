# k8s-tools kit

Installs **kubectl**, **k3d**, and **skaffold** into `~/.local/bin` inside a
sandbox. Architecture-aware (amd64 / arm64) and idempotent — re-running skips
anything already installed.

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

> The kit's own `network.allowedDomains` block is metadata; the effective
> egress policy comes from sbxw's `network_allow`. Keep the two in sync.

## Manual apply (existing sandbox)

```sh
sbx kit add <sandbox> path/to/sbxw/assets/k8s-tools
sbx kit validate path/to/sbxw/assets/k8s-tools   # sanity check the spec
```

## Notes

- `spec.yaml` `content` fields do **not** allow `${VAR}` placeholders (only
  `${WORKDIR}`). The install script uses brace-free `$VAR` syntax for that reason.
- `startup` commands are exec-style arrays (`command: ["bash", "..."]`), not
  shell strings.
- Running a k3d cluster also needs Docker-in-Docker with privileged containers
  and image pulls from Docker Hub / quay.io — allow those separately.
