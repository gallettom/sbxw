-- The top bar, in three groups with room between them:
--   left    the app's name (the pages of terminals have their own pager,
--           above the panes, drawn by the core);
--   centre  New, then what needs you (sbx's requests, agents' requests, a
--           plan waiting): quiet until something waits, then amber;
--   right   usage, Help, the Mac, then the daemon's state.
-- Requests, Network (sbx asks), Setup and Jarvis are in the left rail, drawn
-- by the core: the core marks those extras `in_rail` and they are skipped.
-- The core keeps room on the left for macOS's window buttons, measures the
-- bar (data.app.topbar) and marks the extras that do not fit `more`: they
-- go into one menu, "…" and how many, with a dot in the tone of the most
-- urgent thing it holds.

local tones = { up = "green", connecting = "amber", down = "red" }
local words = { up = "connected", connecting = "connecting", down = "offline" }

-- The right group's tools; every other extra is something that needs you
-- (or New), in the centre.
local tools = { usage = 1, setup = 2, help = 3, mac = 4 }

-- Features' buttons and badges (data.app.extras.topbar): the intent of each
-- by id, for the action (filled by render; actions do not get `data`).
local extra_intents = {}
-- The data of the last render (actions do not get it).
local last = nil

-- An extra's colour by meaning while quiet, inline or in the "⋯" menu.
local function meaning_of(id)
  return (id == "create" and "green") or ((id == "approvals" or id == "relay" or id == "plan") and "blue") or nil
end

-- The usage chip with a small bar per window (the core says when it fits:
-- app.topbar.usage_bars): the label, a 40 by 4 pt bar filled to the share
-- (blue under 70%, amber from 70%, red from 90%), the number. Else its
-- words alone. shell/bar.rs measures it the same way.
local function usage_tone(p)
  if p >= 90 then return "red" elseif p >= 70 then return "amber" end
  return "blue"
end

local function usage_chip(e, u)
  local kids = { key = "extra-topbar-usage", help = e.help, px = 8, gap = 4, radius = "xs", fill = "bg", h = 20 }
  local first = true
  for _, w in ipairs({ { "5h", u.five_hour_pct }, { "7d", u.seven_day_pct } }) do
    local p = tonumber(w[2])
    if p then
      p = math.max(0, math.min(100, p))
      if not first then table.insert(kids, column{ w = 4 }) end
      first = false
      table.insert(kids, text{ w[1], size = "caption", weight = "medium", color = "text-muted" })
      table.insert(kids, progress{ value = p / 100, tone = usage_tone(p), w = 40 })
      table.insert(kids, text{ string.format("%d%%", math.floor(p + 0.5)), size = "caption", weight = "medium", color = p >= 70 and usage_tone(p) or "text-muted" })
    end
  end
  return row(kids)
end

local function extra(e, mac, data)
  local label = e.label or ""
  if e.id == "usage" and ((data or {}).app or {}).topbar and data.app.topbar.usage_bars and data.usage then
    return usage_chip(e, data.usage)
  end
  if e.count then label = (label ~= "" and (label .. " ") or "") .. tostring(e.count) end
  local key = "extra-topbar-" .. e.id
  if not e.intent then
    -- A plain fact (usage): a quiet label, amber when it is high.
    return badge{ label, tone = e.tone == "amber" and "amber" or "neutral", help = e.help, key = key }
  end
  extra_intents[e.id] = e.intent
  local keys = nil
  if e.id == "help" then keys = { "?" } end
  local waits = e.tone == "amber" or e.tone == "red"
  -- Colour by meaning while quiet: New is green (it starts something),
  -- what it goes to blue (requests, a plan), the team purple; tools neutral.
  local meaning = meaning_of(e.id)
  return button{ label, icon = e.icon, key = key, on = "extra", arg = e.id, help = e.help, keys = keys,
    tone = (not waits) and meaning or nil,
    -- Quiet until something waits: then the tone's own colour on its tint.
    variant = (e.id == "create") and "neutral" or "ghost",
    color = waits and e.tone or (e.tone == "blue" and "blue" or nil),
    fill = waits and (e.tone .. "-tint") or nil }
end

-- What does not fit in the bar (the core marks it `more`, by priority:
-- what needs you stays inline longest) goes into one menu.
local function more_menu(hidden)
  local items, tone = {}, nil
  for _, e in ipairs(hidden) do
    -- An icon-only extra is named by its help, up to the first colon, or
    -- its first word when that is still long ("Help and keyboard shortcuts").
    local label = e.label
    if label == "" then
      label = string.match(e.help or "", "^([^:]+)") or e.id
      if #label > 18 then label = string.match(label, "^(%S+)") or label end
    end
    if e.count then label = label .. " (" .. tostring(e.count) .. ")" end
    -- The same colour as inline: its own tone when something waits, else its meaning.
    local t = (e.tone == "amber" or e.tone == "red") and e.tone or meaning_of(e.id)
    table.insert(items, { id = e.id, label = label, icon = e.icon, tone = t, help = e.help ~= "" and e.help or nil })
    if e.intent then extra_intents[e.id] = e.intent end
    if e.tone == "amber" then tone = "amber"
    elseif e.count and tone == nil then tone = "blue" end
  end
  return row{ gap = 4,
    menu{ tostring(#hidden), id = "extra-topbar", icon = "more", items = items, on = "extra_more", variant = "ghost", help = #hidden .. " more: what does not fit in the bar" },
    tone and dot{ tone = tone } or false,
  }
end

-- The modes, after the mark: GUI | TUI (Sessions, the readable view of
-- every session, or the terminals), Lead Mode (only the team leaders,
-- in either) and the Kanban (the Dashboard's tickets, over either). They
-- change what the pane area shows, so they live here and not in the rail,
-- whose entries show more. Compact (a narrow window): icons alone, the
-- words in their tooltips. shell/bar.rs `modes_w` measures the same parts.
local function kanban_button(app, compact)
  local on = ((app.overlay or {}).screen) == "kanban"
  local keys = app.platform == "macos" and { "⌃", "⌥", "K" } or { "Alt", "K" }
  return button{ (not compact) and "Kanban" or nil, icon = "kanban", key = "top-kanban", on = "kanban", radius = "pill", keys = keys,
    tone = "purple", variant = on and "neutral" or "ghost", color = on and "purple" or "text-muted", fill = on and "purple-tint" or nil,
    help = on and "Kanban: shown. Press to close it" or "Kanban: the tickets planned, in progress, in test, ready to merge and merged" }
end

local function modes(app, compact)
  local gui = app.base ~= nil and app.base ~= false
  local lead = app.leads or {}
  local on = lead.on == true
  local keys = app.platform == "macos" and "⌃⌥I" or "Alt+I"
  -- One switch (switch.rs): a track with both labels, a thumb that slides
  -- under the chosen one; a press anywhere flips it.
  return row{ key = "top-modes", gap = 8,
    select{ id = "top-view", switch = true, value = gui and "gui" or "tui", on = "view",
      items = { { id = "gui", label = (not compact) and "GUI" or "", icon = "inbox" }, { id = "tui", label = (not compact) and "TUI" or "", icon = "terminal" } },
      help = (gui and "GUI: Sessions, every session's conversation in one view" or "TUI: the terminals") .. ". Press, or " .. keys .. ", for the other" },
    button{ (not compact) and (on and "Lead Mode On" or "Lead Mode Off") or nil, icon = "leader", key = "top-lead", on = "lead", radius = "pill",
      tone = "purple", variant = on and "neutral" or "ghost", color = on and "purple" or "text-muted", fill = on and "purple-tint" or nil,
      disabled = (not on) and lead.can ~= true,
      help = on and "Lead Mode On: only the team leaders (their terminals, their sessions and threads). Press for everything again"
        or (lead.can == true and "Lead Mode Off: press to show only the team leaders, in the terminals and in Sessions" or "Lead Mode: no team is running") },
    -- The Kanban: quiet at rest, lit only while it is the screen drawn (the
    -- Dashboard's conversation does not light it). A press opens it on the
    -- sandbox in context, or closes it (kanban_toggle).
    kanban_button(app, compact) }
end

return {
  render = function(_, data)
    last = data
    local app = data.app
    local mac = app.platform == "macos"
    -- How the bar fits its width (the core measures it): compact drops the name.
    local fit = app.topbar or {}
    local compact = fit.compact == true
    local centre, right, hidden = { gap = 8 }, { gap = 4 }, {}
    for _, e in ipairs((app.extras or {}).topbar or {}) do
      -- New sits in the sidebar's header while the sidebar shows.
      if e.in_sidebar or e.in_rail then
      elseif e.more then table.insert(hidden, e)
      elseif tools[e.id] then table.insert(right, extra(e, mac, data))
      else table.insert(centre, extra(e, mac, data)) end
    end
    -- The tools in a fixed order: usage, Setup, Help, the Mac.
    local tl = {}
    for _, e in ipairs((app.extras or {}).topbar or {}) do
      if tools[e.id] and not e.more and not e.in_rail then table.insert(tl, e) end
    end
    table.sort(tl, function(a, b) return tools[a.id] < tools[b.id] end)
    right = { gap = 4 }
    for _, e in ipairs(tl) do table.insert(right, extra(e, mac, data)) end
    if #hidden > 0 then table.insert(right, more_menu(hidden)) end
    local state = app.connection.state
    -- The daemon's state: a dot and a word; where it is connected, or why
    -- not, in its tooltip.
    table.insert(right, row{ key = "daemon-state", gap = 8, px = 8, h = 28, help = app.connection.text,
      dot{ tone = tones[state] or "neutral" },
      text{ words[state] or state, size = "caption", color = state == "up" and "text-muted" or (tones[state] or "text-muted") },
    })
    return row{ gap = 16, w = "full", h = "full",
      row{ gap = 12,
        -- The sbxw mark, not the word: the app icon's green prompt and blue
        -- cursor (packaging/make-icon.py), drawn as vectors straight on the
        -- bar, no tile and no border (it is not a button), on the bar's 28 pt
        -- line at the icons' mark size (theme.icon.mark, 24: a little larger
        -- and heavier than the rail's 20 pt glyphs, not by half). The
        -- prompt's icon box is cut to its drawn part so the cursor sits
        -- right after the chevron.
        (not compact) and row{ key = "app-mark", h = 28, gap = 2, align = "center", help = "sbxw",
          column{ w = 13, h = 28, align = "start", justify = "center", icon{ "logo-mark", size = 24, color = "green" } },
          column{ w = 10, h = 28, justify = "end", py = 8, column{ w = 10, h = 3, radius = "mark", fill = "blue" } },
        } or false,
        modes(app, compact),
      },
      spacer{},
      row(centre),
      spacer{},
      row(right),
    }
  end,

  actions = {
    -- GUI or TUI: TUI in TUI does nothing (the shortcut toggles).
    view = function(_, a, v)
      local id = v or a
      local app = (last or {}).app or {}
      local gui = app.base ~= nil and app.base ~= false
      -- GUI also brings Sessions back from under a view drawn over it.
      if id == "gui" then return intent("monitor_open", {}) end
      if gui then return intent("monitor_toggle", {}) end
    end,
    lead = function() return intent("team_leads", { show = "toggle" }) end,
    kanban = function() return intent("kanban_toggle", {}) end,
    extra = function(_, id)
      local i = extra_intents[id]
      if i then return intent(i.name, i.args) end
    end,
    extra_more = function(_, _, id)
      local i = extra_intents[id]
      if i then return intent(i.name, i.args) end
    end,
  },
}
