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

return {
  render = function(_, data)
    local app = data.app
    local j = data.jarvis or { open = false, pending = {}, voice = {} }
    local n = #j.pending
    local key = app.platform == "macos" and "⌘J" or "F7"
    return row{ gap = 14, w = "full", h = "full",
      text{ "sbxw", weight = "bold", size = "lg", color = "text-bright" },
      select{ id = "layout", items = layouts, value = current(app.layout), on = "layout", help = "How many terminals show" },
      spacer{},
      button{ n > 0 and ("Jarvis · " .. n .. " to review") or "Jarvis", icon = j.voice.recording and "mic" or "sparkle",
        key = "jarvis-toggle", on = "jarvis", variant = (j.open or n > 0) and "primary" or "neutral",
        help = (j.open and "Close Jarvis (" or "Open Jarvis (") .. key .. ")" },
      spacer{},
      row{ gap = 6, max_w = 520, help = app.connection.text,
        dot{ tone = tones[app.connection.state] or "neutral" },
        text{ app.connection.state == "up" and app.connection.text or app.connection.state, size = "sm", color = "text-muted", truncate = true },
      },
    }
  end,

  actions = {
    layout = function(_, _, id)
      local panes = tonumber(string.sub(id, 1, 1))
      return intent("set_layout", { panes = panes, split = string.sub(id, 2, 2) == "r" and "rows" or "columns" })
    end,
    jarvis = function() return intent("toggle_jarvis_panel") end,
  },
}
