-- The top bar (the web UI's header): the pane-count switch, Jarvis's
-- button, and where the app is connected. The core keeps room on the left
-- for macOS's window buttons; this screen draws to the right of it.

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
local badge_tones = { blue = "blue", green = "green", amber = "amber", red = "red", purple = "purple" }

-- Features' buttons and badges (data.app.extras.topbar): the intent of each
-- by id, for the action (filled by render; actions do not get `data`).
local extra_intents = {}

local function extra(e)
  local label = e.label or ""
  if e.count then label = (label ~= "" and (label .. " ") or "") .. tostring(e.count) end
  if e.intent then
    extra_intents[e.id] = e.intent
    return button{ label, icon = e.icon, key = "extra-topbar-" .. e.id, on = "extra", arg = e.id, help = e.help,
      -- The tone's own colour (an amber "needs you" reads amber, not green); red is danger.
      variant = e.tone == "red" and "danger" or "neutral",
      color = (e.tone ~= "muted" and e.tone ~= "red" and e.tone ~= "") and e.tone or nil }
  end
  return badge{ label, tone = badge_tones[e.tone] or "neutral", help = e.help, key = "extra-topbar-" .. e.id }
end

-- What does not fit in the bar (the core marks it `more`, by priority:
-- what needs you stays inline longest) goes into one menu, "…" and how
-- many, with a dot in the tone of the most urgent thing it holds.
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
    table.insert(items, { id = e.id, label = label, icon = e.icon })
    if e.intent then extra_intents[e.id] = e.intent end
    if e.tone == "amber" then tone = "amber"
    elseif e.count and tone == nil then tone = "blue" end
  end
  return row{ gap = 4,
    menu{ tostring(#hidden), id = "extra-topbar", icon = "more", items = items, on = "extra_more", variant = "ghost" },
    tone and dot{ tone = tone } or false,
  }
end

return {
  render = function(_, data)
    local app = data.app
    local j = data.jarvis or { open = false, pending = {}, voice = {} }
    local n = #j.pending
    local key = app.platform == "macos" and "⌘J" or "F7"
    -- How the bar fits its width (the core measures it): compact drops the
    -- title and draws the layout switch as an icon menu.
    local fit = app.topbar or {}
    local compact = fit.compact == true
    local extras = { gap = 8 }
    local hidden = {}
    for _, e in ipairs((app.extras or {}).topbar or {}) do
      if e.more then table.insert(hidden, e) else table.insert(extras, extra(e)) end
    end
    if #hidden > 0 then table.insert(extras, more_menu(hidden)) end
    local switch
    if compact then
      local icon = "single"
      for _, l in ipairs(layouts) do if l.id == current(app.layout) then icon = l.icon end end
      switch = menu{ id = "layout", icon = icon, items = layouts, on = "layout", variant = "ghost", help = "How many terminals show" }
    else
      switch = select{ id = "layout", items = layouts, value = current(app.layout), on = "layout", help = "How many terminals show" }
    end
    return row{ gap = 16, w = "full", h = "full",
      (not compact) and text{ "sbxw", weight = "bold", size = "lg", color = "text-bright" } or false,
      switch,
      spacer{},
      row(extras),
      button{ n > 0 and ("Jarvis · " .. n .. " to review") or "Jarvis", icon = j.voice.recording and "mic" or "sparkle",
        key = "jarvis-toggle", on = "jarvis", variant = (j.open or n > 0) and "primary" or "neutral",
        help = (j.open and "Close Jarvis (" or "Open Jarvis (") .. key .. ")" },
      -- The daemon state takes the rest, at the right end, and keeps its dot
      -- and at least a word; its whole line is the tooltip.
      row{ gap = 8, min_w = 96, grow = 1, justify = "end", help = app.connection.text,
        dot{ tone = tones[app.connection.state] or "neutral" },
        text{ app.connection.state == "up" and app.connection.text or app.connection.state, size = "sm", color = "text-muted", truncate = true, max_w = 500 },
      },
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
