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

return {
  render = function(_, data)
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
        -- The sbxw mark, not the word: the app icon's dark tile, its green
        -- prompt and blue cursor (packaging/make-icon.py), drawn as vectors.
        (not compact) and row{ key = "app-mark", w = 22, h = 22, radius = 6, fill = "bg", border = "border", gap = 0, align = "center", justify = "center", help = "sbxw",
          icon{ "logo-prompt", size = 14, color = "green" },
          column{ w = 6, h = 14, justify = "end", py = 2, column{ w = 6, h = 2, radius = 1, fill = "blue" } },
        } or false,
      },
      spacer{},
      row(centre),
      spacer{},
      row(right),
    }
  end,

  actions = {
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
