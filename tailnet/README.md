# sbxw-tailnet

The Tailscale node of sbxw's remote access. A small Go program, built on
Tailscale's official [`tsnet`](https://pkg.go.dev/tailscale.com/tsnet) library,
that joins your tailnet as its own machine (named `sbxw` by default) and
forwards HTTPS on that machine's port 443 to the sbxw daemon's remote listener
on loopback (`remote_addr`, default `127.0.0.1:7683`).

It is the only door from outside. Nothing else on the Mac is reachable through
it: it is a separate node, not the Mac's own Tailscale, and it has no route to
anything but the one loopback port. Nothing needs to be installed on the Mac
besides sbxw.

The daemon starts and stops it; you never run it by hand. The contract with the
daemon is in [docs/security.md, Remote access](../docs/security.md#remote-access), sub-section "The Tailscale node".

## What it serves, and what it does not

**Serves:** TLS on TCP 443 of its tailnet address, with the tailnet's own
certificate (`sbxw.<tailnet>.ts.net`, from Let's Encrypt through Tailscale),
reverse-proxied to the one upstream. HTTP/1.1 only.

**Does not serve, anywhere:** port 80 (no redirect: a phone that types
`http://` gets no answer, which is clearer than a redirect that could be
mistaken for a fallback), any other port, SSH, Taildrop, Taildrive, the
Tailscale web client (5252), an exit node, subnet routes, app connectors, a
peer relay, Funnel or Serve.

Every one of those is off twice:

1. **Never enabled in code.** The program never calls `ListenFunnel`,
   `ListenService`, `ListenSSH` or `ListenPacket`, never sets `RunWebClient`,
   `AdvertiseTags` or any route, and never edits the node's preferences.
2. **Compiled out** where Tailscale offers a build tag for it: see
   `build-tags.txt` and [Compiled out](#compiled-out).

**Funnel** (publishing on the public Internet) needs a call to `ListenFunnel`,
a serve configuration with `AllowFunnel`, and a `funnel` attribute in the
tailnet policy. This program makes none of them, compiles Serve and Funnel
out, and tsnet itself clears any persisted serve configuration the first time
the node comes up. So nothing is reachable from outside your tailnet.

## No auth key, ever

The first join is a login link (Q24). The program never asks for, accepts or
stores an auth key, and **refuses to start** if any `TS_*` or `TSNET_*`
environment variable is set: tsnet reads auth keys, OAuth secrets, a control
server URL and debug switches from them, some before `main` runs. The daemon
starts it with a cleared environment.

## The status file

The program writes one JSON object to `--status-file` (`0600`, in a `0700`
directory, written to a temporary file and renamed, rewritten every 5 s):

```json
{"state": "needs_login", "name": null,
 "login_url": "https://login.tailscale.com/a/...",
 "detail": "Open the link to add this Mac's sbxw node to your tailnet.",
 "pid": 1234, "updated_ms": 1763650000000}
```

| `state` | Meaning |
|---|---|
| `starting` | The node is coming up. |
| `needs_login` | Open `login_url` to add the node to your tailnet (or approve it in the admin console when device approval is on; then `login_url` is null). |
| `running` | Serving at `https://<name>`. |
| `https_off` | MagicDNS or HTTPS certificates are off in the tailnet, or no certificate was issued. `detail` says which and where to turn it on. Nothing is served: without HTTPS the phone's microphone does not work, so there is **no plain-HTTP fallback**. |
| `error` | `detail` says what. Also shown, while still trying, when no login link arrives within `--login-wait` (60 s): the coordination server may be unreachable. |
| `stopped` | Written on a clean stop, then the file is removed. |

The login link is a credential for a few minutes: it is written to the status
file only, never to stdout, stderr or a log. The file never holds a key.

## The proxy

- Forwards to exactly `--forward`, which must be a loopback `IP:PORT` literal
  (`127.0.0.0/8` or `[::1]`); a host name, any other address, and the daemon's
  UI and agent ports (7681, 7682) are refused at start. Whatever the request
  names, the transport dials only that address, and never an environment proxy.
- **Host:** a request whose `Host` is not exactly the tailnet name (or the name
  with `:443`) gets `421`. The upstream receives `Host: <tailnet name>`.
- **Stripped** from every request: `Forwarded`, every `X-Forwarded-*`,
  `X-Real-Ip`, `X-Client-Ip`, `True-Client-Ip`, every `X-Sbxw-*`, every
  `Tailscale-*`, and the hop-by-hop headers (and any header `Connection` names).
- **Then set:** exactly one `X-Forwarded-For: <peer tailnet IP>` (the daemon
  uses it for rate limits) and `X-Sbxw-Tailnet-Node: <peer node DNS name>`
  (from Tailscale's WhoIs). Both are **information, never authentication**: the
  daemon uses the first only as a rate-limit hint and, in step 1, does not read
  the second (a later journal may). A device is trusted only
  for its pairing cookie. The tailnet user's login (an email address) is not
  forwarded.
- A connection from an address WhoIs does not know gets `403`.
- No path rewriting, no normalisation: the daemon receives the path as sent.
- `Upgrade` (websocket) and `CONNECT` get `400`.
- **Limits:** request headers 16 KiB (`431` above), body 8 MiB (`413`; the
  daemon's largest route takes 7,681,024 bytes), header read 10 s, body read
  120 s, idle connection 120 s, upstream answer 5 min.
- Server-sent events stream: responses are flushed at once, no buffering, no
  write timeout.
- An upstream that does not answer gives `502` with a fixed sentence; no
  address, path or error text is returned or logged.

## Logs

stderr only, a line per state change (`state running`, `forgotten`...).
Never a URL (the login link included), a request path or query, a header
value or a body. stdout stays empty.

Tailscale's own logs: backend logs are discarded, user logs (where tsnet would
print the login link) are dropped, and **upload to `log.tailscale.com` is
off** twice: the `ts_omit_logtail` build tag and Tailscale's
`TS_NO_LOGS_NO_SUPPORT` switch, set by the program at start. Consequence:
Tailscale support has no client logs for this node.

## State, and "Forget this node"

`--state-dir` (`~/.sbxw/remote/tailnet`, `0700`; the program refuses a path
that is not absolute or does not end in `remote/tailnet`) holds:

- `tailscaled.state`: the machine key and the **node key** (secrets: whoever
  has them is this node in your tailnet), the node's preferences and profile;
- `certs/`: the ACME account key, the TLS certificate and its private key;
- `tailscaled.log.conf`: a log identity tsnet writes even with uploads off
  (nothing is uploaded), and small cache files.

`sbxw-tailnet forget --state-dir <dir> --status-file <file>` (the daemon runs
it, with the node stopped first) logs the node out at Tailscale when it can
within 15 s, which expires its key, then deletes the whole state directory and
the status file.

The node also appears as a machine in your tailnet's admin console. To remove
it there: <https://login.tailscale.com/admin/machines>, find `sbxw`, the `...`
menu, **Remove**, then **Remove machine**. Do it after "Forget" if the machine
is still listed (a logged-out machine may stay listed; not verified here).

Node keys expire (180 days by default, set in the admin console). The status
then turns to `error` saying the node was logged out; turning remote access off
and on shows a new login link.

## Certificate names are public

Every certificate is logged in the public Certificate Transparency logs, with
the machine's full name (`sbxw.<tailnet>.ts.net`). Hence the neutral default
`--hostname sbxw`. Do not choose a name that says anything about you or your
work. If the name is taken in your tailnet, Tailscale gives `sbxw-1` and the
status file shows the real name.

## Build

```sh
scripts/build-tailnet.sh                 # this machine
scripts/build-tailnet.sh darwin/arm64    # an Apple-silicon Mac
scripts/build-tailnet.sh darwin/amd64    # an Intel Mac
```

The output is `target/tailnet/<goos>-<goarch>/sbxw-tailnet`. The daemon looks
for it **next to its own executable** (no `PATH` lookup): copy it beside
`sbxw`.

- Go version: the `go` line of `go.mod`, with `GOTOOLCHAIN=local` (no
  toolchain download).
- Modules: pinned by `go.sum`, built with `-mod=readonly`.
- Measured here (linux/arm64 sandbox, Go 1.27.4): darwin/arm64 19.1 MB,
  darwin/amd64 20.5 MB; about 30 s cold, a few seconds warm. Two cold builds
  gave the same SHA-256.
- `CGO_ENABLED=0`, `-trimpath`, `-ldflags "-s -w -buildid="`, the tags of
  `build-tags.txt`. Cross-compiling for macOS from Linux works (pure Go).

The Rust workspace does not build or need it.

Tests: `go vet ./... && go test ./...` (`-short` skips the tests that build and
run the binary). No test uses a real tailnet.

## Compiled out

Tailscale's `ts_omit_*` build tags remove whole features from the binary.
`build-tags.txt` lists the ones used; each was checked to build for
darwin/arm64 and linux/arm64. Compiled out: log upload (`logtail`), Tailscale
SSH, Taildrop, Taildrive, the web client, running and using an exit node,
advertising and using subnet routes, app connectors, the peer relay server,
network flow logs, packet capture, client auto-update, full remote
configuration by the tailnet admin, port listing, Wake-on-LAN, the outbound
SOCKS/HTTP proxy, Kubernetes and AWS integration, layer-2 (tap), BGP (bird),
device posture, conn25, identity federation and OAuth auth keys (no auth key
path at all), control-to-node requests (`c2n`), the PeerAPI server and client,
NAT-PMP/UPnP port mapping (no talk to your router; peers that cannot connect
directly go through Tailscale's DERP relays, still end-to-end encrypted),
captive portal probing, and the doctor.

**Could not be compiled out:**

- **Serve and Funnel** (`ts_omit_serve`): the certificate code needs it, the
  build fails without it. The program never calls `ListenFunnel` or
  `ListenService`, and tsnet clears any stored serve configuration at first
  start (see above).
- The WireGuard data path, DERP, MagicDNS, ACME certificates, tailnet lock:
  the node needs them.

**What the node opens on the Mac itself:** two UDP sockets (IPv4 and IPv6, a
random port) for WireGuard and NAT traversal, as every Tailscale node does.
No TCP port on the Mac: the HTTPS listener exists only inside the node's
userspace network stack, on its tailnet address. Checked here with `ss` while
the binary ran.

## Licences

No copyleft licence. Checked from the licence file of every module linked
into the darwin/arm64 build (`go list -deps`), at the versions in `go.sum`:

| Licence | Modules |
|---|---|
| BSD-3-Clause | tailscale.com v1.104.7 (tsnet), golang.org/x/{crypto,exp,net,sync,sys,term,text,time}, filippo.io/edwards25519, go-json-experiment/json, hdevalence/ed25519consensus, tailscale/hujson, tailscale/peercred, go4.org/netipx, and the Go standard library |
| Apache-2.0 | gvisor.dev/gvisor, klauspost/compress, golang/groupcache, google/btree, pires/go-proxyproto, go4.org/mem |
| MIT | tailscale/wireguard-go, fxamacker/cbor, gaissmai/bart, mitchellh/go-ps, x448/float16 |
| ISC | coder/websocket |

A binary release must ship these notices (BSD, MIT, ISC and Apache-2.0 all ask
for the copyright notice and licence text to travel with binaries).
