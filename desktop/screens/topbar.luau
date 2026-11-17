-- The top bar, in three groups with room between them:
--   left    the app's name and how many terminals show;
--   centre  New, then what needs you (sbx's requests, agents' requests, a
--           plan waiting): quiet until something waits, then amber;
--   right   usage, Setup, Help, the Mac, Jarvis, then the daemon's state.
-- The core keeps room on the left for macOS's window buttons, measures the
-- bar (data.app.topbar) and marks the extras that do not fit `more`: they
-- go into one menu, "…" and how many, with a dot in the tone of the most
-- urgent thing it holds.

local layouts = {
  { id = "1", label = "1 terminal", icon = "single" },
  { id = "2c", label = "2 side by side", icon = "columns" },
  { id = "2r", label = "2 stacked", icon = "rows" },
  { id = "3", label = "3 terminals", icon = "grid" },
  { id = "4", label = "4 terminals", icon = "grid" },
}

local function current(layout)
  if layout.panes == 2 then return layout.split == "rows" and "2r" or "2c" end
  return tostring(layout.panes)
end

local tones = { up = "green", connecting = "amber", down = "red" }
local words = { up = "connected", connecting = "connecting", down = "offline" }

-- The right group's tools; every other extra is something that needs you
-- (or New), in the centre.
local tools = { usage = 1, setup = 2, help = 3, mac = 4 }

-- Features' buttons and badges (data.app.extras.topbar): the intent of each
-- by id, for the action (filled by render; actions do not get `data`).
local extra_intents = {}

local function extra(e, mac)
  local label = e.label or ""
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
  return button{ label, icon = e.icon, key = key, on = "extra", arg = e.id, help = e.help, keys = keys,
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
    table.insert(items, { id = e.id, label = label, icon = e.icon, help = e.help ~= "" and e.help or nil })
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
    local j = data.jarvis or { open = false, pending = {}, voice = {} }
    local n = #j.pending
    local mac = app.platform == "macos"
    -- How the bar fits its width (the core measures it): compact drops the
    -- name and draws the layout switch as an icon menu.
    local fit = app.topbar or {}
    local compact = fit.compact == true
    local centre, right, hidden = { gap = 8 }, { gap = 4 }, {}
    for _, e in ipairs((app.extras or {}).topbar or {}) do
      if e.more then table.insert(hidden, e)
      elseif tools[e.id] then table.insert(right, extra(e, mac))
      else table.insert(centre, extra(e, mac)) end
    end
    -- The tools in a fixed order: usage, Setup, Help, the Mac.
    local tl = {}
    for _, e in ipairs((app.extras or {}).topbar or {}) do
      if tools[e.id] and not e.more then table.insert(tl, e) end
    end
    table.sort(tl, function(a, b) return tools[a.id] < tools[b.id] end)
    right = { gap = 4 }
    for _, e in ipairs(tl) do table.insert(right, extra(e, mac)) end
    if #hidden > 0 then table.insert(right, more_menu(hidden)) end
    local switch
    if compact then
      local icon = "single"
      for _, l in ipairs(layouts) do if l.id == current(app.layout) then icon = l.icon end end
      switch = menu{ id = "layout", icon = icon, items = layouts, on = "layout", variant = "ghost", help = "How many terminals show" }
    else
      switch = select{ id = "layout", items = layouts, value = current(app.layout), on = "layout", help = "How many terminals show" }
    end
    local state = app.connection.state
    table.insert(right, button{ n > 0 and ("Jarvis · " .. n .. " to review") or "Jarvis", icon = j.voice.recording and "mic" or "sparkle",
      key = "jarvis-toggle", on = "jarvis", variant = (j.open or n > 0) and "primary" or "neutral",
      keys = mac and { "⌘", "J" } or { "F7" },
      help = j.open and "Close Jarvis" or "Open Jarvis" })
    -- The daemon's state: a dot and a word; where it is connected, or why
    -- not, in its tooltip.
    table.insert(right, row{ key = "daemon-state", gap = 8, px = 8, h = 28, help = app.connection.text,
      dot{ tone = tones[state] or "neutral" },
      text{ words[state] or state, size = "caption", color = state == "up" and "text-muted" or (tones[state] or "text-muted") },
    })
    return row{ gap = 16, w = "full", h = "full",
      row{ gap = 12,
        (not compact) and text{ "sbxw", weight = "semibold", size = "title", color = "text-bright" } or false,
        switch,
      },
      spacer{},
      row(centre),
      spacer{},
      row(right),
    }
  end,

  actions = {
    layout = function(_, _, id)
      local panes = tonumber(string.sub(id, 1, 1))
      return intent("set_layout", { panes = panes, split = string.sub(id, 2, 2) == "r" and "rows" or "columns" })
    end,
    jarvis = function() return intent("toggle_jarvis_panel") end,
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
