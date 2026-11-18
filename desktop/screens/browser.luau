-- The Browser panel (docked on the right): the sandbox's own Chromium,
-- live. The web UI's assets/js/browser.js.
--
-- The picture and the person's pointer and keys on it are the core's
-- (`surface{ id = "browser" }`): this screen draws only the chrome around
-- it, from `data.browser_ui`. What the sandbox says (an address, a title,
-- a host, an error) is drawn as text, nothing else. An address typed here is
-- sent to the sandbox's browser; it is never opened on this machine.
-- Allowing a refused host returns an intent the core confirms in its dialog.

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

-- The address bar follows the page until something is typed in it, and again
-- once it is sent (the web UI's browserShowUrl).
local function bar_value(state, ui)
  if state.typing then return state.typed or "" end
  return ui.url or ""
end

return {
  init = function() return { typing = false, typed = "" } end,

  render = function(state, data)
    local ui = data.browser_ui or {}
    local surf = data.app.surface or {}
    local release = surf.release or ui.release_key or "Ctrl+Esc"
    local status = ui.status or "idle"
    if status == "idle" then
      return column{ pad = 12, gap = 8, w = "full",
        note("No browser shown. Open one from a sandbox's pane bar (Browser): it runs inside that sandbox, under its network policy."),
      }
    end
    local sb = ui.sandbox or "?"
    -- The browser could not start (an error before the first picture) or
    -- has ended: no picture, no live controls, the message and what to do next.
    local dead = status == "ended" or (ui.error ~= nil and status ~= "live")
    local live = (status == "live" or status == "starting") and not dead

    local top = row{ gap = 8, w = "full",
      text{ sb, weight = "semibold", color = "text-bright", truncate = true, grow = 1, help = ui.title },
      button{ "Hide", on = "hide", variant = "ghost", key = "browser-hide", help = "Hide the browser: it keeps running for the agent" },
      button{ status == "closing" and "Closing..." or "Close browser", on = "close", variant = "danger", key = "browser-close", help = "Close the sandbox's browser; then choose what to do with the hosts allowed from here" },
    }

    local nav = row{ gap = 4, w = "full",
      button{ "←", on = "back", variant = "ghost", key = "browser-back", help = "Back", disabled = not live },
      button{ "→", on = "forward", variant = "ghost", key = "browser-forward", help = "Forward", disabled = not live },
      button{ icon = "reload", on = "reload", variant = "ghost", key = "browser-reload", help = "Reload", disabled = not live },
      input{ id = "browser-url", value = bar_value(state, ui), placeholder = "Address, sent to the sandbox's browser", on = "typed", on_submit = "go", grow = 1, disabled = not live },
      select{ id = "browser-viewport", value = ui.viewport or "auto", items = ui.viewports or {}, on = "viewport", disabled = not live },
    }

    local out = { gap = 8, w = "full", h = "full", pad = 8, top, nav }

    -- Hosts this sandbox's network policy refused while the page loaded.
    local blocked = ui.blocked or {}
    if #blocked > 0 then
      local chips = { gap = 4, w = "full", text{ "Blocked by this sandbox's network policy", size = "xs", color = "amber", weight = "semibold" } }
      for _, b in ipairs(blocked) do
        table.insert(chips, row{ gap = 8, w = "full", pad = 4, radius = "xs", fill = "amber-tint", key = "browser-blocked-" .. b.host,
          text{ b.host, mono = true, size = "sm", color = "text-bright", truncate = true, grow = 1 },
          b.fixed and text{ b.fixed, size = "xs", color = "text-muted" }
            or button{ "Allow", on = "allow", arg = b.host, key = "browser-allow-" .. b.host, help = "Allow " .. b.host .. " for " .. sb .. " only" },
          button{ icon = "close", on = "ignore", arg = b.host, variant = "ghost", key = "browser-ignore-" .. b.host, help = "Leave " .. b.host .. " blocked" },
        })
      end
      table.insert(out, column(chips))
    end

    local n = ui.allowed or 0
    table.insert(out, text{ n > 0
      and (n .. " host" .. (n == 1 and "" or "s") .. " allowed from here, for this sandbox only. Closing the browser asks whether to keep " .. (n == 1 and "it" or "them") .. ".")
      or "Runs inside the sandbox, under its network policy.", size = "xs", color = "text-dim", wrap = true, key = "browser-grant-note" })

    if ui.error then
      table.insert(out, column{ w = "full", pad = 8, radius = "sm", fill = "red-tint", border = "red-border", key = "browser-error",
        text{ ui.error, size = "sm", color = "red", wrap = true } })
    elseif status == "starting" then
      table.insert(out, note("Starting the browser in the sandbox..."))
    end

    if dead then
      -- The next step, through what the app already has: the Kits panel on
      -- this sandbox's sbxw.toml (the kits line), and the sandbox's actions
      -- (Recreate, behind the core's dialog). Nothing here changes anything by itself.
      table.insert(out, row{ gap = 8, w = "full", key = "browser-next",
        button{ "Kits…", icon = "box", on = "kits", arg = sb, key = "browser-kits",
          help = "Open the Kits panel on this sandbox's sbxw.toml: add \"sbxw:agent-skills\" to its kits line" },
        button{ "Recreate…", icon = "more", on = "actions", arg = sb, key = "browser-recreate",
          help = "Open " .. sb .. "'s actions: Recreate applies a new kits line (the app asks before it does anything)" },
      })
      return column(out)
    end

    -- The picture: the core's, growing into what is left.
    table.insert(out, column{ grow = 1, w = "full", radius = "sm", border = ui.focused and "blue" or "elevated", fill = "floor",
      surface{ id = "browser" } })

    local stats = ui.stats or {}
    local hint = ui.focused
      and ("Keys go to the page. F6: the address bar. " .. release .. ": give the keyboard back.")
      or "Click the page to type in it."
    table.insert(out, row{ gap = 8, w = "full",
      text{ hint, size = "xs", color = ui.focused and "blue" or "text-dim", truncate = true, grow = 1, key = "browser-hint" },
      (stats.frames or 0) > 0 and text{ (stats.fps or 0) .. " fps" .. (stats.latency_ms and (" · " .. stats.latency_ms .. " ms") or ""), size = "xs", color = "text-dim",
        help = "Pictures a second from the sandbox, and the time from a click or a key to the next picture (median)", key = "browser-stats" } or false,
    })
    return column(out)
  end,

  actions = {
    hide = function() return intent("browser_hide", {}) end,
    close = function() return intent("browser_close", {}) end,
    back = function() return intent("browser_back", {}) end,
    forward = function() return intent("browser_forward", {}) end,
    reload = function() return intent("browser_reload", {}) end,
    typed = function(state, _, value) state.typing = true; state.typed = value end,
    go = function(state, _, value)
      state.typing = false
      state.typed = ""
      if value == nil or value == "" then return nil end
      return intent("browser_nav", { url = value })
    end,
    viewport = function(_, _, value) return intent("browser_viewport", { value = value }) end,
    kits = function(_, sb) return intent("kits_show", { sandbox = sb }) end,
    actions = function(_, sb) return intent("open_modal", { screen = "sandbox-actions", arg = sb }) end,
    allow = function(_, host) return intent("browser_allow", { host = host }) end,
    ignore = function(_, host) return intent("browser_ignore", { host = host }) end,
  },
}
