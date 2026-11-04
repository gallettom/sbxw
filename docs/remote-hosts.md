# Remote hosts (EXPERIMENTAL)

[← Back to README](../README.md)

> **EXPERIMENTAL.** Remote hosts are tested against a fake `ssh`, and
> `sbxw host check` passed 16/16 on a real Ubuntu 26.04 host on 2026-11-04.
> That host had no KVM, so no sandbox was created there: nothing has been run
> on a real KVM host. To check a real host, use `sbxw host check <name>` or
> `scripts/remote-smoke.sh <name>`. Read [What is not tested](#what-is-not-tested)
> before relying on it.

sbxw can drive `sbx` on another machine over SSH. Sandboxes on that host show
up beside your local ones, and you work with them the same way. A remote sandbox
is named `<host>~<name>`, for example `buildbox~neos`; a bare `neos` stays local.

```bash
sbxw host add buildbox --ssh buildbox        # once, confirms the host key
sbxw host check buildbox                     # no sandbox needed
sbxw up buildbox~neos --path /home/me/code   # --path is a path on the host
```

## Requirements

On the host:

- Ubuntu 24.04 or newer, with KVM (`/dev/kvm`) and your user in the `kvm` group.
- `sbx` 0.50 or newer, installed and signed in (`sbxw host provision` does both).
- The **first-run network policy question answered once**: `sbx` asks it on
  the first `sbx run`. Run `sbx run` (or create any sandbox) by hand on the host
  and answer it. `sbx ls` does not ask. sbxw closes stdin on remote commands, so
  an unanswered question should fail rather than wait; this is untested on a
  real host.
- `git` (sbxw reads workspaces with it).

On your machine: `ssh`, with a key that works without a passphrase prompt
(an agent is fine). sbxw runs ssh with `BatchMode=yes`, so it never asks for
a password.

## Add a host

```bash
sbxw host add buildbox --ssh buildbox
sbxw host add lab --host 10.0.0.5 --user tom --port 2222 --key ~/.ssh/id_ed25519
```

Use `--ssh` for an alias from `~/.ssh/config`, or spell the host out. `--key` is
a path to an identity file, never the key itself.

**Host keys are checked strictly.** `host add` fetches the host's key with
`ssh-keyscan`, shows its SHA256 fingerprint and asks you to confirm; only on
yes is it written to `~/.ssh/known_hosts`. Compare the fingerprint with the one
on the host (`ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub`) before saying
yes. After that ssh refuses a changed key, and so does sbxw: it never accepts a
new key on its own. `-y` skips the question for scripts; only use it when you
verified the fingerprint some other way.

Other commands: `sbxw host ls`, `sbxw host edit`, `sbxw host rm`, `sbxw host provision`. See the
[CLI reference](cli.md#remote-hosts).

## Provisioning

`sbxw host provision <name>` takes a plain Ubuntu host to "sbx installed, signed
in, network policy set". Run it in your own terminal: sudo on the host asks for
its password there, and sbxw never sees it.

```bash
sbxw host add buildbox --ssh buildbox
sbxw host provision buildbox --dry-run    # what it would do
sbxw host provision buildbox
```

Each step prints `✓`, `-` (skipped, with the reason) or `✗`, and it stops at the
first `✗` with what to do.

1. **Checks, before anything is installed.** Ubuntu 24.04 or newer; x86-64 or
   arm64; `/dev/kvm` present; your user in the `kvm` group. Without `/dev/kvm`
   it explains nested virtualization and stops (on AWS you need an Intel
   instance type with the NestedVirtualization CPU option, C8i, M8i or R8i at
   least; t3a cannot). If only the group is missing it offers
   `sudo usermod -aG kvm <user>`, then asks you to reconnect and run it again,
   because a group change only reaches new logins.
2. **Install sbx** unless `sbx version` already answers 0.50.0 or newer. The
   commands are Docker's documented ones, run over `ssh -t`:
   `curl -fsSL https://get.docker.com | sudo REPO_ONLY=1 sh`, then
   `sudo apt install -y docker-sbx`.
3. **Sign in.** `sbx` has no read-only command that says whether it is signed
   in (an unauthenticated `sbx ls` just lists nothing), so sbxw asks. If you say
   yes it explains what a PAT (Personal Access Token) is (Docker Hub, Account
   settings, Personal access tokens), asks for your Docker username and the
   token without echo, and pipes the token to
   `sbx login --username <u> --password-stdin --no-input` on the host. The token
   travels on stdin only: never in an argument, a file or a log, and sbxw does
   not keep it.
4. **Network policy.** You pick Open, Balanced (default) or Locked Down, and
   sbxw runs `sbx policy init allow-all|balanced|deny-all --no-input`, which
   Docker's CLI reference documents as the way to set it without the first-run
   prompt. If the host already has a policy the step is skipped; to change one,
   `sbx policy reset` on the host clears it.
5. **`sbxw host check <name>`**, and its verdict.

Flags: `--yes` takes the safe defaults (installs, Balanced) and never supplies
the token, so it skips sign-in; `--skip-install` fails if sbx is missing instead
of installing it; `--dry-run` still runs the read-only probes over ssh (it needs
them to know what a real run would do) and prints every command that would
change the host instead of running it.

Verified from Docker's published CLI reference (2026-11-04): `sbx login
--username/--password-stdin`, `sbx policy init`, and the global `--no-input`.
Not verified on a real host: the whole run, how `sbx policy init` words its
refusal when a policy is already set (sbxw skips on the word "already"; any
other failure is shown with the command to run by hand), and whether
`sbx login` is needed before `sbx policy init` on a new install.

## `trust_secrets`

By default a host is **not** trusted with secrets. With `--trust-secrets`
(or `sbxw host edit <host> --trust-secrets`), sbxw may send to that host:

- the Anthropic token (`CLAUDE_CODE_OAUTH_TOKEN` or API key), on stdin of the
  command that needs it;
- the OAuth kit (your Claude login) and the `my-skills` kit.

Without it, none of these are sent, so a remote sandbox there has no Claude
login until you trust the host or sign in inside it. What is never sent, trusted
or not: your SSH keys, the rest of `~/.sbxw`, and your environment. Secrets
travel on stdin only, never in a command line, a file on the host's disk set up
by sbxw, or a log. Other kits are copied as they are.

Only turn it on for a machine you control: anyone with root on the host can read
what a sandbox there holds.

## Start a sandbox

```bash
sbxw up buildbox~neos --path /home/me/code
sbxw ls
sbxw logs buildbox~neos      # as far as remote sandboxes are wired in this build
```

The path is on the host. sbxw keeps one SSH connection per host
(a ControlMaster), so commands do not pay a handshake each time. The same
connection carries a `-R` tunnel from the host's `127.0.0.1:<agent-port>` back
to this machine's agent listener, which is how a sandbox there reaches sbxw.
The agent port defaults to 7682; change it with `--agent-port` if something on
the host already uses it.

Kits you use are copied to `~/.sbxw/kits/<hash>/` on the host, once per content:
an unchanged kit is not sent again, a changed one gets a new directory.

## Ports

A port a remote sandbox publishes is forwarded to your machine with `ssh -L`,
bound to `127.0.0.1` only (never your network). The local port is the same
number when it is free, else the next free one; `sbxw ports-ls` shows the pair.

## When the host is unreachable

"Host unreachable" is its own state, not "sandbox gone". When the connection
drops (a laptop that sleeps, a network change, the host rebooting):

- the host's sandboxes stay listed with their **last known state**;
- actions on them are refused with a clear message instead of failing oddly;
- sbxw reconnects in the background with a growing delay, and everything comes
  back by itself, including the port forwards on the same local ports.

## Check a host: `sbxw host check`

```bash
sbxw host check buildbox
sbxw host check buildbox --keep     # leave the temp dir on the host
```

Run it after `host add`. It needs no KVM and **creates no sandbox**. It works
in a `mktemp -d` directory on the host (removed at the end) and uses its own
connection, so a running daemon keeps its link. One line per step:

```
  ✓ ssh: connects, `--` before the command: echo ok came back
  ✓ ssh: hostile words come back unchanged: 13 words, byte for byte
  ✓ sbx: version: 0.51.0 (needs 0.50.0 or newer)
  ✓ link: ControlMaster comes up
  ✓ link: -L forward reaches a service on the host: 127.0.0.1:8123 -> host port 8123
  - link: -R tunnel ...: skipped, bash not found
  ✓ kits: mirror copies once, a second run copies nothing: ...
  ✓ git: init on the host, scan finds the repo: found proj
PASS: 14 passed, 1 skipped, 0 failed
```

A step is `✓`, `✗` with the reason, or `-` skipped with the reason (for example
no `python3` on the host for the `-L` test). The exit code is 0 only when
nothing failed. If the link does not come up, the usual cause is a daemon already
linked to that host (it holds the agent port there); run `sbxw down` first.

`scripts/remote-smoke.sh <host>` runs `sbxw host test` and then `sbxw host check`,
and ends by listing what it did not check.

## What is not tested

Nobody has run these on a real KVM host yet:

1. **Creating a remote sandbox** (`sbxw up host~name`) against a real `sbx`.
   Everything around it is tested against fakes.
2. **The interactive terminal**: `ssh -tt <host> sbx run …` and what a terminal
   does through it (resize, Ctrl-C, a dropped connection).
3. **The sandbox → host → your machine leg of the `-R` tunnel.** `host check`
   proves host → your machine only; a sandbox's own network path to the host's
   loopback depends on `sbx`.
4. **`sbxw host provision` end to end**: the sudo install over `ssh -t`, the PAT
   sign-in and `sbx policy init`. Its logic is unit-tested against a fake host;
   the commands come from Docker's published docs.

If you run it on a KVM host, please report what worked and what did not. Until
then remote hosts stay marked EXPERIMENTAL here and in `sbxw host --help`.
