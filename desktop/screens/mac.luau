-- The Virtual Mac panel (docked on the right): the web UI's Setup, Mac
-- (assets/js/mac.js). One card over `data.mac` (GET /api/mac, the whole
-- answer: `{ ok, mac }`): installed or not, the VM's state, the lease, the
-- buttons; then what waits on you, the guest's one-time setup, the timers.
--
-- Install, create, shut down, force off and take back return intents the
-- core confirms in its own dialog. A request's reason is the sandbox's own
-- text: drawn as text, under a label naming that sandbox.

local tones = { running = "green", paused = "amber", starting = "blue", stopping = "blue", stopped = "neutral", missing = "neutral", unknown = "neutral" }

local steps = {
  "In the VM window, finish macOS Setup Assistant and create the guest's user account. Sign in to no personal or work account: every sandbox shares this guest.",
  "In the guest, open Finder, then /Volumes/My Shared Files/tools, then TerminalMacGuest.app.",
  "Use its two buttons to grant Screen Recording and Accessibility in the guest's System Settings (reopen it if macOS asks).",
  "Approve its login item if macOS asks, and keep the guest logged in.",
  "Turn on automatic login for that account (System Settings, Users & Groups; FileVault must be off in the guest), so a VM sbxw boots reaches the desktop and its helper without you.",
  "Stop the guest from sleeping or locking: in System Settings, Lock Screen, set \"Turn display off when inactive\" and \"Require password after the screen saver begins or display is turned off\" to Never. A guest whose display sleeps comes back on its lock screen, where the agent cannot go on.",
}

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function pill(tone, label, help)
  return row{ gap = 4, dot{ tone = tone }, text{ label, size = "sm", color = "text" }, help = help }
end

local function card(children, border)
  children.gap = children.gap or 8
  children.w = "full"
  children.pad = 12
  children.radius = "md"
  children.fill = "surface-alt"
  children.border = border or "elevated"
  return column(children)
end

local function btn(label, on, variant, busy)
  return button{ label, on = on, variant = variant or "neutral", disabled = busy, key = "mac-" .. on }
end

local function n(v)
  return v ~= nil and tostring(v) or "?"
end

return {
  render = function(_, data)
    local answer = data.mac or {}
    local m = answer.mac or {}
    local err = (data.app.errors or {}).mac
    local out = { gap = 12, w = "full", pad = 12 }

    table.insert(out, note("A real macOS guest, run on this Mac by terminal-mac, that sandboxes with the sbxw:terminal-mac kit can drive: Safari, Mac apps, native windows. One sandbox at a time. Starting, resuming, lending a VM sbxw did not start, and publishing a port need your yes; clicks and typing do not."))

    if err or answer.ok == false then
      table.insert(out, note("Could not read the Mac's state: " .. (answer.error or err or "unknown error"), "red"))
    end
    if m.supported == nil then
      if not err and answer.ok ~= false then table.insert(out, note("Reading the Mac's state...")) end
      return scroll{ column(out) }
    end
    if not m.supported then
      table.insert(out, card{ pill("neutral", "not available"),
        note("The virtual Mac needs an Apple silicon Mac (macOS 14 or later). This host is not one, so the mac_* tools say so and do nothing.") })
      return scroll{ column(out) }
    end

    local busy = m.busy ~= nil
    if busy then
      table.insert(out, column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "blue-tint", key = "mac-busy",
        text{ m.busy .. "...", size = "sm", color = "text-bright", wrap = true },
        progress{ value = 0.5, tone = "blue" } })
    end
    if m.error then
      table.insert(out, column{ w = "full", pad = 8, radius = "sm", fill = "red-tint", border = "red-border", key = "mac-error",
        text{ m.error, size = "sm", color = "red", wrap = true } })
    end

    if not m.installed then
      table.insert(out, card{
        row{ gap = 8, w = "full", text{ "terminal-mac", weight = "semibold", color = "text-bright" }, spacer{}, pill("neutral", "not installed") },
        note("sbxw installs release " .. n(m.pinned) .. " under ~/.sbxw/terminal-mac, its SHA-256 checked. It is an early release candidate from one author, self-signed and not notarised by Apple."),
        row{ btn("Install terminal-mac", "install", "primary", busy) },
      })
      table.insert(out, button{ "Refresh", on = "refresh", variant = "ghost", key = "mac-refresh" })
      return scroll{ column(out) }
    end

    local st = m.state or "unknown"
    local running = st == "running"
    local stopping = st == "stopping"
    local pills = { gap = 12, pill(tones[st] or "neutral", st) }
    if running then
      table.insert(pills, m.guest_ready and pill("green", "guest ready") or pill("amber", "guest not ready", "Finish the one-time setup below"))
    end
    table.insert(pills, m.holder and pill("purple", "held by " .. m.holder) or pill("neutral", "free"))

    local acts = { gap = 8, w = "full" }
    if not running and not stopping and st ~= "starting" then
      local label = (st == "paused" and "Resume") or (st == "missing" and "Create and start") or "Start"
      table.insert(acts, btn(label, "up", "primary", busy))
    end
    -- A shutdown that hangs is usually a dialog in the guest: show it.
    if running or st == "paused" or stopping then
      table.insert(acts, button{ "Open its window", on = "window", disabled = busy, key = "mac-window", help = "terminal-mac shows the VM's own window on this Mac: the live screen, with your mouse and keyboard" })
    end
    if running then table.insert(acts, btn("Pause", "pause", "neutral", busy)) end
    if m.holder then table.insert(acts, btn("Take it back from " .. m.holder, "take_back", "neutral", busy)) end

    local ends = { gap = 8, w = "full" }
    if running or st == "paused" then table.insert(ends, btn("Shut down", "stop", "danger", busy)) end
    if stopping then table.insert(ends, btn("Force off", "force_off", "danger", busy)) end
    table.insert(ends, spacer{})
    table.insert(ends, button{ "Refresh", on = "refresh", variant = "ghost", key = "mac-refresh" })

    local ports = {}
    for _, p in ipairs(m.ports or {}) do table.insert(ports, p) end
    table.insert(out, card{
      row{ gap = 8, w = "full", text{ "VM", color = "text-muted" }, text{ n(m.vm), mono = true, weight = "semibold", color = "text-bright", truncate = true, grow = 1 } },
      row(pills),
      row{ gap = 8, w = "full", text{ "Release", size = "sm", color = "text-muted", w = 60 }, text{ n(m.pinned), size = "sm", mono = true } },
      row{ gap = 8, w = "full", align = "start", text{ "Ports", size = "sm", color = "text-muted", w = 60 },
        #ports > 0 and text{ table.concat(ports, "  "), size = "sm", mono = true, wrap = true, grow = 1 } or text{ "none published", size = "sm", color = "text-dim" } },
      row(acts),
      row(ends),
    })

    -- Cards waiting on you. sbxw's sentence and the sandbox's reason are
    -- separate fields and drawn apart, so a reason cannot pass for sbxw's words.
    local pending = m.pending or {}
    if #pending > 0 then
      local c = { border = "amber", text{ "Waiting on you", weight = "semibold", color = "text-bright" } }
      for _, p in ipairs(pending) do
        table.insert(c, column{ gap = 4, w = "full", key = "mac-pending-" .. n(p.id),
          text{ p.action or "", weight = "semibold", size = "sm", wrap = true },
          p.reason and text{ "Reason given by " .. n(p.sandbox) .. " (its own words, not checked by sbxw)", size = "xs", color = "text-muted", wrap = true } or note(n(p.sandbox) .. " gave no reason."),
          p.reason and column{ w = "full", pad = 8, radius = "xs", fill = "surface", text{ p.reason, size = "sm", mono = true, wrap = true } } or false,
          row{ button{ "Approve or refuse in Requests", on = "request", arg = p.id, key = "mac-request-" .. n(p.id) } },
        })
      end
      table.insert(out, card(c, "amber"))
    end
    if st == "missing" then
      table.insert(out, note("Starting creates the VM: macOS is downloaded from Apple (several GB) and installed, then you finish its setup once."))
    end
    if running and not m.guest_ready then
      local c = { border = "amber", key = "mac-steps", text{ "One-time guest setup", weight = "semibold", color = "text-bright" } }
      for i, s in ipairs(steps) do
        table.insert(c, row{ gap = 8, w = "full", align = "start", text{ i .. ".", size = "sm", color = "text-muted", w = 16 }, text{ s, size = "sm", wrap = true, grow = 1 } })
      end
      table.insert(c, note("This panel notices on its own when the guest is ready."))
      table.insert(out, card(c, "amber"))
    end
    local s = m.settings or {}
    table.insert(out, note("A lease lapses after " .. n(s.idle_release_minutes) .. " min idle. A VM sbxw started is paused after " ..
      n(s.pause_after_minutes) .. " min unheld and shut down after " .. n(s.stop_after_minutes) ..
      " min, forced if the shutdown still hangs " .. n(s.force_stop_after_minutes) .. " min later. Set these under [mac] in sbxw.toml.", "text-dim"))
    return scroll{ column(out) }
  end,

  actions = {
    install = function() return intent("mac_install", {}) end,
    up = function() return intent("mac_up", {}) end,
    pause = function() return intent("mac_pause", {}) end,
    window = function() return intent("mac_window", {}) end,
    stop = function() return intent("mac_stop", {}) end,
    force_off = function() return intent("mac_force_off", {}) end,
    take_back = function() return intent("mac_take_back", {}) end,
    refresh = function() return intent("mac_refresh", {}) end,
    request = function(_, id) return intent("relay_open", { id = id }) end,
  },
}
