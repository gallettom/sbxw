# `sbxw:terminal-mac`: the virtual Mac

[← Back to Kits and skills](../../docs/kits-and-skills.md)

- [Why](#why)
- [How it fits](#how-it-fits)
- [Setting it up](#setting-it-up)
- [The agent's side](#the-agents-side)
- [Your side](#your-side)
- [Configuration](#configuration)
- [Trust](#trust)
- [Acceptance checklist](#acceptance-checklist)

## Why

A sandbox is a Linux container. Its headless Chromium covers most web checks,
but some things only macOS can show: a page in **Safari/WebKit**, a **Mac
app** (the Dynamic Island, say), a native window. Until now the agent asked
you for a screenshot. With the virtual Mac, it looks for itself.

The Mac is [terminal-mac](https://github.com/govijr/terminal-mac): a real
macOS guest under Apple Virtualization, with a helper *inside the guest* that
captures its screen and posts its input. It never touches your own desktop,
pointer or clipboard.

## How it fits

```
sandbox ──mac_* tool──▶ sbxw daemon (host) ──terminal-mac CLI──▶ macOS guest
   ▲                         │
   └──── screenshot ◀────────┘     boot / port ──▶ 🧑 approval card
```

- **This kit** (opt-in) turns on the `mac_*` tools of the sandbox's
  `sbxw-relay` MCP server and adds `/mac`. Nothing is installed in the
  sandbox, and it needs no network rule.
- **The daemon** runs terminal-mac on the host. Only an **Apple silicon Mac**
  (macOS 14 or later) can. On any other host, including a remote Linux host,
  the tools say so and do nothing.
- **One VM for every sandbox**, held by one sandbox at a time (a *lease*).

The code: `crates/core/src/mac.rs` (rules: pin, lease, lifecycle, argument
checks), `crates/daemon/src/svc/mac.rs` (running terminal-mac, approvals),
`crates/daemon/src/http/mac.rs` (routes), `assets/relay-mcp.js` (the tools),
`assets/js/mac.js` (Setup → Mac), `crates/cli/src/cmd/mac.rs` (`sbxw mac`).

## Setting it up

1. Add the kit to the project's `sbxw.toml`, next to the defaults:
   ```toml
   kits = ["sbxw:codemap", "sbxw:agent-skills", "sbxw:md-to-pdf-tools", "sbxw:retro", "sbxw:terminal-mac"]
   ```
   Then `sbxw up <name>` (an existing sandbox gets it through `sbx kit add`).
2. Install terminal-mac and start the VM: **Setup → Mac → Install**, then
   **Start**. Or, in a terminal, `sbxw mac up` (it installs first if needed).
   The first start downloads macOS from Apple (several GB) and creates the VM.
3. **Once per VM**, finish the guest's setup in its window (**Open its
   window**, or `sbxw mac show`):
   1. Finish Setup Assistant and create the guest's user account. Sign in to
      **no** personal or work account.
   2. In the guest, open Finder → `/Volumes/My Shared Files/tools` →
      `TerminalMacGuest.app`.
   3. Use its two buttons to grant **Screen Recording** and **Accessibility**.
   4. Approve its login item if macOS asks, and stay logged in.
   5. Turn on **automatic login** for that account (System Settings → Users &
      Groups; FileVault must be off in the guest). Without it, a VM sbxw boots
      stops at the login screen: the helper does not run, the agent cannot act,
      and a shutdown request can hang in `stopping`.
   6. Stop the guest from sleeping or locking: System Settings → Lock Screen,
      set *Turn display off when inactive* and *Require password after the
      screen saver begins or display is turned off* to **Never**. A display
      that sleeps comes back on the lock screen, and the agent's tools report
      `No guest display is available` until then.

   The panel shows *guest ready* once both permissions are granted and the
   guest is logged in. Count
   about 15 minutes, plus the download.

## The agent's side

| Tool | What it does |
|---|---|
| `mac_start` | Takes the lease. Starting, creating or resuming the VM needs your approval first, and so does lending a running VM that sbxw did not start or resume itself. Later leases on a VM sbxw brought up need none. |
| `mac_open_port` | Publishes one of the sandbox's ports where the guest can reach it, after your approval, and returns the URL to open **in the guest**. |
| `mac_screenshot` | The guest's screen, inline in the tool result and saved in the workspace as `.sbxw-artifacts/mac-shots/mac-*.png` (the newest 40), which the Project panel opens on. `~/.sbxw/shots/` if the workspace cannot be written. |
| `mac_click`, `mac_move`, `mac_drag`, `mac_scroll`, `mac_type`, `mac_key` | Input in the guest. `screenshot: true` returns the result in the same call. |
| `mac_status`, `mac_release` | Where it stands; let go. |

A call from a sandbox that does not hold the lease takes it if it is free and
sbxw brought the VM up (or you approved lending it), and is refused if another
sandbox holds it. A lease nobody uses lapses after `idle_release_minutes`. One
sandbox holds it for 60 minutes at most in one go: after that, another sandbox
that asks takes it over. Releasing and taking it again at once does not reset
those 60 minutes.

The daemon never writes into a sandbox: the screenshot travels in the reply
and the sandbox's MCP server writes the file, as with
[screenshot requests](../../docs/relay.md#screenshot-requests).

## Your side

- **Approval cards.** A sandbox asking to boot, create or resume the VM, to
  be lent a VM you started yourself, or to publish a port,
  opens a *Virtual Mac request* in the browser popup and the Island. The card
  says what will happen first (for a VM that does not exist yet: that macOS is
  downloaded, several GB). Then, in a paragraph of its own headed *Reason given
  by* the sandbox, comes the agent's reason, with its line breaks and control
  characters removed and cut at 300 characters, so it cannot pass for sbxw's
  words. **Setup → Mac** lists the same cards with the reason in its own
  block. **Approve** runs
  it, and the agent is told what came of it (the URL, or the error). **Refuse**
  starts nothing.
- **No approval for clicks and typing.** The guest is disposable and shared;
  that isolation is the protection, not a click per action.
- **Setup → Mac** shows the VM's state, the guest checklist, who holds the
  lease, the ports opened for it, and buttons to start, pause, shut down, open
  its window, or **take the lease back**.
- **Automatic pause and stop.** A VM sbxw started that nobody holds is paused
  after `pause_after_minutes` (its RAM stays reserved; resuming is quick) and
  shut down normally after `stop_after_minutes`. If that shutdown still hangs
  in `stopping` `force_stop_after_minutes` later, sbxw forces it
  (`terminal-mac stop --force`): with 0.2.0-6, a normal stop was seen to hang
  for good, keeping the VM's RAM. Only a shutdown sbxw asked for is forced,
  and what is lost is only what was unsaved in the disposable guest.
  **Force off** in the panel (offered after 2 minutes in `stopping`) and
  `sbxw mac force-off` do it at once. A guest
  still being set up is not paused, and a VM you started yourself outside sbxw
  is left alone until a sandbox has used it. Ports opened for the guest are
  unpublished when the VM stops.

From a terminal:

```bash
sbxw mac install   # the pinned terminal-mac, SHA-256 checked
sbxw mac up        # start (creates the VM on first use)
sbxw mac status
sbxw mac show      # the VM's window
sbxw mac pause
sbxw mac stop      # normal shutdown (forced if it hangs)
sbxw mac force-off # power off at once; unsaved guest work is lost
sbxw mac release   # take the lease back from a sandbox
```

## Configuration

In the `sbxw.toml` the **daemon** was started with (one VM per host, not per
project). Every key is optional:

```toml
[mac]
vm = "default"               # the terminal-mac VM to drive
idle_release_minutes = 10    # an unused lease lapses
pause_after_minutes = 15     # an unheld VM sbxw started is paused
stop_after_minutes = 120     # …then shut down normally
force_stop_after_minutes = 3 # a shutdown still hanging this long is forced
guest_host_ip = ""           # where the guest reaches published ports; empty = bridge100's address
```

`SBXW_TERMINAL_MAC=/path/to/terminal-mac` makes the daemon use another
terminal-mac (your own install, or a fake in tests).

## Trust

- **The release is pinned.** sbxw installs terminal-mac `0.2.0-6` from its
  GitHub release, checks the archive's SHA-256 against the hash compiled into
  sbxw, and runs the archive's own installer into `~/.sbxw/terminal-mac`.
  Nothing goes to `~/.local/bin` or your shell configuration. Moving the pin
  is an sbxw release.
- **It is an early release candidate** from one author, self-signed and not
  notarised by Apple. Review it before use on machines that hold sensitive
  work, and follow your organisation's software approval process.
- **The guest is shared** by every sandbox that has the kit. Never sign it in
  to a personal or organisation account, and never put secrets in it.
- **The guest has its own internet access**, through the VM's NAT on the
  host. The sandbox's network allowlist does not govern it. A sandbox holding
  the Mac can open any site in the guest's browser and read the pages back
  through screenshots, including sites its own allowlist blocks. Enable the
  kit only where that is acceptable.
- **A published port is reachable on the VM bridge** (`192.168.64.x`), not on
  your LAN. It is removed when the lease of the sandbox that asked for it ends
  (released, lapsed, taken over, taken back, or the sandbox stopped or
  removed), and with the VM when it stops. The next sandbox to hold the Mac
  cannot open it.
- **A VM you started yourself is not lent without asking.** The first lease
  on a running VM that sbxw did not start or resume needs your approval, and
  so does resuming a paused VM. Sign out of anything personal before you
  approve.

## Acceptance checklist

The tests drive a fake terminal-mac; this is the check on a real Mac. Every
item has to pass before the feature is called done.

1. `sbxw mac install` on a clean Apple silicon Mac installs `0.2.0-6` under
   `~/.sbxw/terminal-mac/bin/terminal-mac`. A tampered archive (wrong hash)
   installs nothing.
2. `sbxw mac up` with no VM creates one, and the window opens. After the six
   setup steps, **Setup → Mac** shows *guest ready* without a reload.
3. `terminal-mac status default` and `terminal-mac action default status`
   return JSON that sbxw reads as `running` and *guest ready*. If not, adjust
   `parse_vm_state` / `guest_ready` in `crates/core/src/mac.rs` to the real
   shape.
4. In a sandbox with the kit, with the VM stopped: `mac_start` opens a card;
   **Refuse** starts nothing; a second `mac_start` and **Approve** boots it and
   the agent gets the lease.
5. `mac_screenshot` shows the guest's desktop; `mac_key cmd+space`,
   `mac_type Safari`, `mac_key return` open Safari.
6. A dev server on `0.0.0.0:5173` in the sandbox: `mac_open_port 5173` and
   **Approve** return `http://192.168.64.1:5173` (or similar); Safari in the
   guest loads the app.
7. A second sandbox's `mac_start` says the Mac is in use; **Take it back** in
   the panel frees it.
8. Leave it idle: it pauses after 15 minutes and shuts down after 2 hours
   (shorten both in `[mac]` to test), and the published port disappears from
   `sbx ports`. A shutdown still in `stopping` 3 minutes later is forced.
9. The Island shows the card with **Approve** and **Refuse** (its Swift
   changes were not compiled in the sandbox that wrote them).
10. After item 6, `mac_release`: port 5173 disappears from `sbx ports` at
    once. Publish it again and stop the sandbox instead: within a minute the
    lease is free and the port is gone.
11. With the VM paused, `mac_start` opens a *Resume* card. Start the VM
    yourself (`terminal-mac start default`) and restart the daemon:
    `mac_start` opens a card saying sbxw did not start it, and a click before
    that is refused.
12. A card whose reason holds line breaks shows it on one line under
    *Reason given by*, in the Island, the popup and **Setup → Mac**.
