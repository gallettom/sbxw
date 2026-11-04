# Remote hosts (EXPERIMENTAL)

[← Back to README](../README.md)

> **EXPERIMENTAL.** Remote hosts have been tested against a fake `ssh` and,
> without sandboxes, against a real Ubuntu host. They have **not** been run on a
> real KVM host. Expect rough edges, and read [What is not tested](#what-is-not-tested)
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
- `sbx` 0.50 or newer, installed and signed in.
- The **first-run network policy question answered once**: run `sbx ls` on the
  host by hand and choose a policy. `sbx` asks it interactively; over a
  non-interactive SSH connection it cannot, so sbxw gets an error (never a hang)
  until it has been answered.
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

Other commands: `sbxw host ls`, `sbxw host edit`, `sbxw host rm`. See the
[CLI reference](cli.md#remote-hosts).

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

If you run it on a KVM host, please report what worked and what did not. Until
then remote hosts stay marked EXPERIMENTAL here and in `sbxw host --help`.
