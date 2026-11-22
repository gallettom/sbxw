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
- **Then set:** exactly one `X-Forwarded-For: <peer tailnet IP>`, which the
  daemon uses only as a rate-limit hint, **never as authentication**: a device
  is trusted only for its pairing cookie. **No identity is forwarded**: no
  user, login (an email address) or machine name. What WhoIs says stays inside
  this program, for the owner check below, and is never logged.
- **Only the person's own devices get in** (see below). Any other peer gets
  `403 Forbidden.` with no detail, before anything is forwarded.
- No path rewriting, no normalisation: the daemon receives the path as sent.
- `Upgrade` (websocket) and `CONNECT` get `400`.
- **Limits:** request headers 16 KiB (`431` above), body 26 MiB (`413`; the
  daemon's largest route, a phone's file upload, takes 25 MiB; it was 8 MiB
  before that route), streamed to the daemon, header read 10 s, body read
  120 s (kept by the person's decision: a 25 MiB file needs about 1.7 Mbit/s
  of uplink; on a slower link the upload is cut at 120 s, fails, and is
  retried), idle connection
  120 s, upstream answer 5 min.
- Server-sent events stream: responses are flushed at once, no buffering, no
  write timeout.
- An upstream that does not answer gives `502` with a fixed sentence; no
  address, path or error text is returned or logged.

## Only the person's own devices

Being on the tailnet is not enough: a tailnet can hold other people's devices
(a family or work tailnet), servers with tags, and machines shared in from
other tailnets. On every request the program asks Tailscale's `WhoIs` who is
behind the connection and forwards only when all of these hold:

- WhoIs answers, with both a node and a user profile;
- the node has **no ACL tag** (`Node.Tags` empty): a tagged node belongs to
  the tailnet, not to a person;
- the node was **not shared in** (`Node.Sharer` is zero);
- the node's owner (`Node.User`) and the profile (`UserProfile.ID`) are both
  the **same stable numeric user id** as this sbxw node's owner
  (`Status.Self.UserID`, read once when the node comes up). Login names and
  display names are never compared.

The sbxw node itself is untagged and owned by whoever opened the login link;
if it has no owner id (tagged, or unknown), it refuses to serve at all.

"The person's own devices" therefore means, in Tailscale's terms: the
untagged machines logged in with the same Tailscale account as the one that
added sbxw. A refused request writes one stderr line at most once a minute,
`refused a connection from a device that is not the node owner's`, without the
peer's name or address.

**Not verified without a real tailnet** (the tests use a fake WhoIs):

- that Tailscale fills `Node.User`, `UserProfile.ID` and `Node.Sharer` this
  way for a shared-in node, for a user of another tailnet who was invited as a
  member, and for a tagged node in practice;
- how a device behaves when the person logs it in with a second Tailscale
  account: it is a different user id and is refused, which is intended.

For defence in depth the person can also limit port 443 of `sbxw` to their own
devices in the tailnet policy (an ACL grant from `autogroup:member` or their
own login to `sbxw:443`); the program does not depend on it.

## Logs

stderr only, a line per state change (`state running`, `forgotten`...).
Never a URL (the login link included), a request path or query, a header
value or a body. stdout stays empty, except with `--push`, where it carries
only the answers to push jobs (see [Push](#push)).

Tailscale's own logs: backend logs are discarded, user logs (where tsnet would
print the login link) are dropped, and **upload to `log.tailscale.com` is
off** twice: the `ts_omit_logtail` build tag and Tailscale's
`TS_NO_LOGS_NO_SUPPORT` switch, set by the program at start. Consequence:
Tailscale support has no client logs for this node.

## Push

With `--push`, the program also sends Web Push notifications to the person's
iPhone, from jobs the daemon writes on its stdin; it answers each job on
stdout. Without `--push` it never reads stdin or writes stdout. The pipe
protocol is in `.sbxw-artifacts/remote/API-push.md`; the code is `push.go`,
Go's standard library only (no new module).

**What it sends:** for each job, one HTTPS `POST` to the subscription's
endpoint at Apple, with a 230-byte body encrypted to the phone's key (RFC 8291,
`aes128gcm`) and a VAPID token signed with the Mac's key (RFC 8292, `sub` = the
project URL given by the daemon, no e-mail). Inside the encryption there is only
`{"kind":"p"|"n","badge":0..99,"name":"<sandbox>","lead":true|false}`: a kind
letter, a count, the sandbox's name (0 to 63 letters, digits and hyphens, else
the job is refused) and whether it is a team leader, padded to 128 bytes so that
every push has the same size whatever the name. No title, message text or device
name. The phone shows a fixed French text naming the sandbox.

**To whom:** Apple's push service only. Apple sees the Mac's public IP, the
endpoint (which identifies the phone's subscription), the time, the fixed size,
the TTL, a fixed `Topic: sbxw` (the same for every push, so Apple keeps one
pending push per phone and learns nothing of the kind) and
the VAPID token; it cannot read the payload.

**What it refuses**, each with a test in `push_test.go`:

- any endpoint that is not `https://<label>.push.apple.com[:443]/<path>` with a
  plain path of at most 512 bytes of `[A-Za-z0-9_\-/.]` and no user info,
  query, fragment, IP literal or trailing dot;
- a host that resolves to loopback, private, link-local, unique-local,
  unspecified, multicast or tailnet (`100.64.0.0/10`) addresses, checked before
  connecting and again in the dialer;
- any method but `POST`, any body or header the daemon would like to choose
  (the job has no such field), redirects (never followed), proxies (its own
  transport ignores `HTTPS_PROXY`, and is not tsnet's dialer), an unverified
  certificate (system roots; no switch to skip the check exists in the binary);
- more than 10 s per push, more than one push at a time, more than 60 jobs an
  hour, a job line over 4 KiB.

Ignoring proxies is deliberate (no general route to the Internet): on a
network where the Internet is reachable only through a proxy, every push fails
and is answered `network`.

A malformed line is answered `bad_job`, never a crash. If the daemon closes
either pipe, pushing stops and the proxy goes on. The VAPID key comes only on
the pipe's first line: never in argv, the environment or a file. Logs name a
job's id, kind and status only; never the endpoint, a key or the token. No
port is opened for this.

Not verified here: Apple's real answers, and delivery to a real iPhone.

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
for it **next to its own executable** (no `PATH` lookup). The script never
copies it there itself; its last lines say where that is and how:

```
sbxw looks for it next to the sbxw on your PATH, in /Users/you/dev/sbxw/target/debug. Put it there:
  cp /Users/you/dev/sbxw/target/tailnet/darwin-arm64/sbxw-tailnet /Users/you/dev/sbxw/target/debug/
```

or `sbxw looks for it at <path>: already in place.` when OUT is that place, or,
with no `sbxw` on `PATH`, the `cp` with "the folder of your sbxw binary". A
build for another machine says to put it next to that machine's `sbxw`.

If it is missing, `sbxw remote on` and `sbxw remote status` name the path the
daemon looked at and the one command that builds it there:

```
sbxw-tailnet is missing: sbxw looks for it at /Users/you/dev/sbxw/target/debug/sbxw-tailnet
It is built by scripts/build-tailnet.sh. From the sbxw source folder, build it in place:
  scripts/build-tailnet.sh darwin/arm64 '/Users/you/dev/sbxw/target/debug/sbxw-tailnet'
```

Set-up, in order (the full steps: [docs/security.md, Remote access](../docs/security.md#remote-access)):

1. **(Mac)** Build it and put it next to `sbxw`, as above.
2. **(Mac)** `sbxw remote on`, then `sbxw remote status`: its `state:` line goes
   `starting`, then `waiting for login` with a `login link:` to open in any
   browser (the Mac's is fine), or `waiting for approval` (approve `sbxw` on the
   Machines page of the admin console), or `https unavailable` (turn on MagicDNS
   and HTTPS Certificates on the DNS page of the admin console), then
   `running at sbxw.<tailnet>.ts.net`.
3. **(Mac)** `sbxw remote pair` prints the link. It opens **only on the phone**:
   the Mac itself is not on the tailnet (only this node is), unless it also
   runs the Tailscale app.
4. **(phone)** Tailscale on, same account; open the whole link (with the part
   after `#`), then type the six digits it shows at the Mac's prompt.

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
