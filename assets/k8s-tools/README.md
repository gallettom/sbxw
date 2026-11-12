# k8s-tools kit

Installs **kubectl**, **k3d**, and **skaffold** into `~/.local/bin` inside a
sandbox. Architecture-aware (amd64 / arm64; kubectl and k3d on 32-bit arm) and
idempotent: re-running skips anything already installed. The three downloads
run side by side; a failed one is reported in the start log and skipped, never
failing the container start.

Each tool is pinned in `files/home/agent/.local/bin/sbxw-install-k8s`: a
version and the hash of what is downloaded (kubectl's client archive from
`dl.k8s.io`, the k3d and skaffold release binaries from GitHub). A download
that does not match is not installed. To update, move a version and its
hashes together; sandboxes pick it up at their next start.

With sbxw's kit cache (`cache = true`, the default) the downloads are made
once per host: later sandboxes copy them from `~/.sbxw/cache`, checked against
the same hashes.

## Usage

Reference the kit *directory* (not a single file) from your `sbxw.toml`:

```toml
kits = [
  "path/to/sbxw/assets/k8s-tools",
]
```

sbxw applies it via `sbx kit add` on every `sbxw up`, **after** the network
policy so the installers have egress access.

## Network

The kit declares what it reaches (`dl.k8s.io`, `cdn.dl.k8s.io`, `github.com`,
`release-assets.githubusercontent.com`, `objects.githubusercontent.com`): its
`network-policy@1` allow rules join the sandbox's policy when it is applied, so
`sbxw.toml` needs no `network_allow` entry for them.

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
