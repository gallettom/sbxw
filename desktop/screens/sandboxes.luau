-- The sandbox list (the web UI's sidebar). Built in; a copy at
-- ~/.sbxw/desktop/screens/sandboxes.luau replaces it and reloads on save.
--
-- Rows are grouped as the web UI groups them: chats first, then one group per
-- workspace folder, then the rest. Clicking a row opens its agent in the
-- focused pane (which starts a stopped sandbox), as the web UI does.

local tones = { running = "green", stopped = "neutral" }

local function basename(path)
  return (path:gsub("/+$", ""):match("([^/]+)$")) or path
end

local function groups(list)
  local chats, by_ws, order, other = {}, {}, {}, {}
  for _, sb in ipairs(list) do
    if sb.chat then
      table.insert(chats, sb)
    elseif sb.workspace and sb.workspace ~= "" then
      if not by_ws[sb.workspace] then
        by_ws[sb.workspace] = {}
        table.insert(order, sb.workspace)
      end
      table.insert(by_ws[sb.workspace], sb)
    else
      table.insert(other, sb)
    end
  end
  table.sort(order, function(a, b) return basename(a) < basename(b) end)
  local out = {}
  if #chats > 0 then table.insert(out, { label = "Chats", items = chats }) end
  for _, ws in ipairs(order) do
    -- A workspace with one sandbox is not a group in the web UI either.
    if #by_ws[ws] > 1 then
      table.insert(out, { label = basename(ws), help = ws, items = by_ws[ws] })
    else
      table.insert(other, by_ws[ws][1])
    end
  end
  if #other > 0 then table.insert(out, { label = nil, items = other }) end
  return out
end

local function in_pane(app, name)
  for _, p in ipairs(app.panes) do
    if p.sandbox == name then return p.index end
  end
  return nil
end

-- Features' buttons and badges on a row (data.app.extras.sandboxes[name]):
-- the intent of each by "<sandbox>\n<id>", for the action.
local extra_intents = {}
local badge_tones = { blue = "blue", green = "green", amber = "amber", red = "red", purple = "purple" }

local function extras_of(sb, app)
  local out = {}
  for _, e in ipairs(((app.extras or {}).sandboxes or {})[sb.name] or {}) do
    local label = e.label or ""
    if e.count then label = (label ~= "" and (label .. " ") or "") .. tostring(e.count) end
    local key = "extra-sandbox-" .. sb.name .. "-" .. e.id
    if e.intent then
      extra_intents[sb.name .. "\n" .. e.id] = e.intent
      if label == "" then
        table.insert(out, button{ icon = e.icon, variant = "ghost", color = e.tone ~= "muted" and e.tone or nil, on = "extra", arg = sb.name .. "\n" .. e.id, help = e.help, key = key })
      else
        table.insert(out, button{ label, icon = e.icon, variant = "ghost", color = e.tone ~= "muted" and e.tone or nil, on = "extra", arg = sb.name .. "\n" .. e.id, help = e.help, key = key })
      end
    else
      table.insert(out, badge{ label, tone = badge_tones[e.tone] or "neutral", help = e.help, key = key })
    end
  end
  return out
end

local function item(sb, app)
  local selected = app.selected == sb.name
  local pane = in_pane(app, sb.name)
  local running = sb.status == "running"
  local busy = sb.status ~= "running" and sb.status ~= "stopped"
  local actions
  if running then
    actions = button{ icon = "stop", on = "stop", arg = sb.name, color = "red", help = "Stop", key = "stop-" .. sb.name }
  elseif busy then
    actions = false
  else
    actions = button{ icon = "play", on = "start", arg = sb.name, color = "green", help = "Start", key = "start-" .. sb.name }
  end
  local extras = extras_of(sb, app)
  local top = { gap = 6, w = "full",
    text{ sb.name, color = "text-bright", weight = "medium", truncate = true, grow = 1 },
  }
  -- A feature that shows sbx's approvals itself replaces the plain flag.
  if #extras == 0 and sb.approvals > 0 then
    table.insert(top, badge{ "⚑ " .. sb.approvals, tone = "blue", help = "sbx waits for you to decide something" })
  end
  for _, x in ipairs(extras) do table.insert(top, x) end
  if pane then table.insert(top, badge{ tostring(pane), tone = "blue", help = "In pane " .. pane }) end
  if not pane then table.insert(top, button{ icon = "plus", variant = "ghost", on = "spare", arg = sb.name, help = "Open in a spare pane", key = "spare-" .. sb.name }) end
  return column{
    key = "row-" .. sb.name, on = "open", arg = sb.name, gap = 3, px = 9, py = 7, radius = "sm", w = "full",
    fill = selected and "surface" or nil, hover = "surface",
    row(top),
    row{ gap = 6, w = "full",
      dot{ tone = tones[sb.status] or "amber" },
      text{ sb.status, size = "sm", color = "text-muted" },
      text{ sb.agent, size = "sm", color = "text-dim", truncate = true, grow = 1 },
      running and button{ icon = "terminal", variant = "ghost", on = "bash", arg = sb.name, help = "Open a shell", key = "bash-" .. sb.name } or false,
      actions,
    },
  }
end

local function matches(sb, filter)
  return filter == "" or string.find(string.lower(sb.name), string.lower(filter), 1, true) ~= nil
end

return {
  init = function()
    return { filter = "" }
  end,

  render = function(state, data)
    local app = data.app
    local shown = {}
    for _, sb in ipairs(data.sandboxes) do
      if matches(sb, state.filter) then table.insert(shown, sb) end
    end
    local body = {}
    for _, g in ipairs(groups(shown)) do
      local rows = { gap = 2, w = "full", py = 4, radius = "md", fill = g.label and "surface-alt" or nil }
      if g.label then
        table.insert(rows, row{ gap = 5, px = 10, py = 2, help = g.help,
          icon{ g.label == "Chats" and "chat" or "folder", size = 11, color = "text-muted" },
          text{ string.upper(g.label), size = "xxs", color = "text-muted", truncate = true },
        })
      end
      for _, sb in ipairs(g.items) do table.insert(rows, item(sb, app)) end
      table.insert(body, column(rows))
    end
    if #data.sandboxes == 0 then
      table.insert(body, text{ app.connection.state == "up" and "No sandboxes yet. Create one with `sbxw up`." or "Waiting for the daemon.", color = "text-dim", size = "sm", wrap = true })
    elseif #shown == 0 then
      table.insert(body, text{ "Nothing matches “" .. state.filter .. "”.", color = "text-dim", size = "sm", wrap = true })
    end
    body.gap = 6
    return column{ gap = 8, h = "full", w = "full",
      row{ gap = 6, w = "full", px = 4,
        text{ "SANDBOXES", size = "sm", weight = "semibold", color = "text" },
        spacer{},
        text{ tostring(#data.sandboxes), size = "sm", color = "text-dim" },
      },
      app.connection.state ~= "up" and row{ gap = 6, w = "full", pad = 8, radius = "sm", fill = "amber-tint", border = "amber",
        icon{ "warning", size = 12, color = "amber" },
        text{ app.connection.text, size = "sm", color = "amber-strong", wrap = true, grow = 1 },
      } or false,
      input{ id = "filter", value = state.filter, placeholder = "Filter", on = "filter" },
      scroll(body),
    }
  end,

  actions = {
    filter = function(state, _, text) state.filter = text or "" end,
    open = function(_, name)
      return { intent("select", { sandbox = name }), intent("open_terminal", { sandbox = name }) }
    end,
    spare = function(_, name)
      return { intent("select", { sandbox = name }), intent("open_terminal", { sandbox = name, pane = "spare" }) }
    end,
    bash = function(_, name)
      return intent("open_terminal", { sandbox = name, mode = "bash", pane = "spare" })
    end,
    start = function(_, name) return intent("start_sandbox", { sandbox = name }) end,
    stop = function(_, name) return intent("stop_sandbox", { sandbox = name }) end,
    extra = function(_, key)
      local i = extra_intents[key]
      if i then return intent(i.name, i.args) end
    end,
  },
}
