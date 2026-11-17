-- The sandbox list: a status rail. Built in; a copy at
-- ~/.sbxw/desktop/screens/sandboxes.luau replaces it and reloads on save.
--
-- Every row has a 3 pt rail on its left edge that carries the sandbox's
-- state by colour AND shape, with the state word beside it:
--   wants you  amber, solid, with a notch     (the only one that draws the eye)
--   working    blue, dashed
--   running    green, solid                   (running and idle)
--   starting…  grey, dashed                   (any state in progress)
--   stopped    grey, hollow
--   failed     red, broken in the middle      (unreachable, or a state sbx calls an error)
-- The name owns the first line. The second: the state word, what needs you
-- (text badges: "sbx asks 3", "asks you 1", "2 threads"), the pane it is in,
-- the agent. Start or stop is always on the right. The other actions (a
-- shell, a spare pane, the ⋯ menu that holds every action of the row) show
-- on hover, on keyboard focus and on the selected row.
--
-- Keys: row-<name>, start-<name>, stop-<name>; the row's menu
-- extra-sandbox-<name> (its list extra-sandbox-<name>-list), whose items are
-- keyed as the inline extras were: extra-sandbox-<name>-<id> (network,
-- threads, more, ...), plus extra-sandbox-<name>-bash (open a shell),
-- -spare (open in a spare pane), -start, -stop; a badge keeps its extra's key
-- when it shows inline; more-<name> is the "+N" of badges that did not fit.
-- Sessions: extra-row-<name>-<id>. Groups: group-<label>.
--
-- Keyboard: Up and Down move between rows, Enter opens the terminal, Space
-- starts or stops.

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
  if #chats > 0 then table.insert(out, { label = "Chats", items = chats, color = chats[1].name }) end
  for _, ws in ipairs(order) do
    -- A workspace with one sandbox is not a group in the web UI either.
    if #by_ws[ws] > 1 then
      table.insert(out, { label = basename(ws), help = ws, items = by_ws[ws], color = by_ws[ws][1].name })
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

-- Features' extras on a row (data.app.extras.sandboxes[name]): the intent of
-- each by "<sandbox>\n<id>", for the actions (filled by render).
local extra_intents = {}
-- Each row's status (for Space, which starts or stops it) and the create
-- feature's "New" intent (the empty list's action), filled by render.
local statuses = {}
local new_intent = nil

local function extras(sb, app)
  return ((app.extras or {}).sandboxes or {})[sb.name] or {}
end

-- What a counted extra says as a text badge (the words the pane bars use).
local function badge_text(e)
  local n = e.count
  if e.id == "approvals" then return "sbx asks " .. (n or "") end
  if e.id == "relay" then return (n or "") .. " asking" end
  if e.id == "threads" then return n and (n .. (n == 1 and " thread" or " threads")) or "threads" end
  if e.id == "kit-add" and n then return n .. (n == 1 and " kit waits" or " kits wait") end
  local label = e.label or ""
  if n then label = (label ~= "" and (label .. " ") or "") .. tostring(n) end
  return label
end

-- An extra's name in the row's menu: its help up to the first colon.
local function menu_label(e)
  local l = e.label or ""
  if l == "" or utf8.len(l) == 1 or l == "⑂" then
    l = string.match(e.help or "", "^([^:]+)") or e.id
  end
  if e.count then l = l .. " (" .. e.count .. ")" end
  return l
end

-- The row's state: its word, the rail's tone and shape.
local function state_of(sb, data)
  if sb.unreachable_since then return { word = "unreachable", tone = "red", shape = "broken" } end
  local st = sb.status
  if st == "stopped" then return { word = "stopped", tone = "muted", shape = "hollow" } end
  if st ~= "running" then
    if string.find(st, "…$") or string.find(st, "%.%.%.$") or string.find(st, "ing$") then
      return { word = st, tone = "muted", shape = "dashed" }
    end
    return { word = st, tone = "red", shape = "broken" }
  end
  local wants, working = false, false
  for _, s in ipairs(data.sessions or {}) do
    if s.sandbox == sb.name then
      if s.state == "attention" then wants = true elseif s.state == "working" then working = true end
    end
  end
  for _, e in ipairs(extras(sb, data.app)) do
    if e.tone == "amber" and e.id ~= "codemap" then wants = true end
  end
  if wants then return { word = "wants you", tone = "amber", shape = "notch" } end
  if working then return { word = "working", tone = "blue", shape = "dashed" } end
  return { word = "running", tone = "green", shape = "solid" }
end

local rail_fill = { green = "green", blue = "blue", amber = "amber", red = "red", muted = "text-muted" }

-- The rail: 3 pt, its shape says the state without colour.
local function rail(tone, shape, small)
  local c = rail_fill[tone] or "text-muted"
  local w = small and 2 or 3
  if shape == "hollow" then
    return column{ w = w, h = "full", radius = 2, border = c }
  end
  if shape == "dashed" then
    local segs = { w = w, h = "full", gap = 4 }
    for _ = 1, small and 2 or 4 do table.insert(segs, column{ w = w, grow = 1, radius = 1, fill = c }) end
    return column(segs)
  end
  if shape == "broken" then
    return column{ w = w, h = "full", gap = small and 4 or 8,
      column{ w = w, grow = 1, radius = 1, fill = c }, column{ w = w, grow = 1, radius = 1, fill = c } }
  end
  if shape == "notch" then
    -- A tab into the row at mid height: amber AND a shape no other state has.
    return row{ h = "full", gap = 0, align = "center",
      column{ w = w, h = "full", radius = 1, fill = c },
      column{ w = 4, h = small and 8 or 14, fill = c } }
  end
  return column{ w = w, h = "full", radius = 2, fill = c }
end

-- About how wide a text is at 12 pt: the UI is monospace, 0.6 em a character.
local function est(t)
  return (utf8.len(t) or #t) * 7.2
end

local word_color = { amber = "amber", blue = "blue", red = "red", green = "text-muted", muted = "text-muted" }

local function item(sb, app, data)
  local name = sb.name
  local selected = app.selected == name
  local pane = in_pane(app, name)
  local running = sb.status == "running"
  local busy = sb.status ~= "running" and sb.status ~= "stopped"
  local st = state_of(sb, data)
  statuses[name] = sb.status

  local menu_items = {}
  if running then
    table.insert(menu_items, { id = "bash", label = "Open a shell", icon = "terminal" })
  end
  table.insert(menu_items, { id = "spare", label = "Open in a spare pane", icon = "plus" })
  -- What needs you, as text badges: amber first, then counts.
  local wants, counts = {}, {}
  for _, e in ipairs(extras(sb, app)) do
    if e.intent then extra_intents[name .. "\n" .. e.id] = e.intent end
    if e.tone == "amber" then table.insert(wants, e) elseif e.count then table.insert(counts, e) end
    if e.intent then
      table.insert(menu_items, { id = e.id, label = menu_label(e), icon = e.icon or (e.id == "threads" and "chat") or (e.id == "approvals" and "flag") or nil })
    end
  end
  if running then
    table.insert(menu_items, { id = "stop", label = "Stop", icon = "stop" })
  elseif not busy then
    table.insert(menu_items, { id = "start", label = "Start", icon = "play" })
  end

  -- Line 2: the state word, the badges that fit whole (never cut: the rest
  -- are "+N", whose tooltip names them), the agent in what is left. Its
  -- right end is kept for the ⋯ menu, so nothing is ever drawn under it.
  local col_w = ((app.sidebar or {}).w or 264) - 79
  -- The ⋯ menu sits at the end of the name's line when the whole name fits
  -- beside it, else at the end of line 2, whose room it then keeps.
  local menu_on_name = (utf8.len(name) or #name) * 8.4 + 8 + 32 <= col_w
  local room = col_w - est(st.word) - 4 - (menu_on_name and 0 or 36)
  local line2 = { gap = 4, w = "full", clip = true,
    text{ st.word, size = "caption", weight = st.tone == "amber" and "semibold" or "normal", color = word_color[st.tone] },
  }
  local all = {}
  for _, e in ipairs(wants) do table.insert(all, e) end
  for _, e in ipairs(counts) do table.insert(all, e) end
  if pane then table.insert(all, { id = "pane", pane = pane }) end
  local rest = {}
  for i, e in ipairs(all) do
    local label = e.pane and ("pane " .. e.pane) or badge_text(e)
    local w = est(label) + 12 + 4
    -- Keep room for a "+N" while something else follows.
    local tail = (i < #all) and (est("+" .. (#all - i)) + 16) or 0
    if #rest == 0 and w + tail <= room then
      room = room - w
      if e.pane then
        table.insert(line2, badge{ label, tone = "blue", help = "Shown in pane " .. e.pane })
      else
        local tone = e.tone == "amber" and "amber" or (e.tone == "red" and "red" or "neutral")
        local key = "extra-sandbox-" .. name .. "-" .. e.id
        if e.intent then
          table.insert(line2, row{ key = key, on = "extra", arg = name .. "\n" .. e.id, help = e.help, hover = "active-bg", radius = "xs", badge{ label, tone = tone } })
        else
          table.insert(line2, badge{ label, tone = tone, help = e.help, key = key })
        end
      end
    else
      table.insert(rest, label)
    end
  end
  if #rest > 0 then
    table.insert(line2, badge{ "+" .. #rest, tone = #wants > 0 and "amber" or "neutral", help = table.concat(rest, ", ") .. " (all in the ⋯ menu)", key = "more-" .. name })
  end
  table.insert(line2, text{ sb.agent, size = "caption", color = "text-muted", truncate = true, grow = 1 })
  if not menu_on_name then table.insert(line2, column{ w = 32 }) end

  -- The row's primary action, always shown, a button at rest: start or stop.
  local primary
  if running then
    primary = button{ icon = "stop", on = "stop", arg = name, help = "Stop " .. name, keys = { "Space" }, key = "stop-" .. name, tab_stop = false, hover = "red-tint" }
  elseif busy then
    primary = column{ w = 28 }
  else
    primary = button{ icon = "play", on = "start", arg = name, help = "Start " .. name, keys = { "Space" }, key = "start-" .. name, tab_stop = false, hover = "green-tint" }
  end

  -- Every action of the row, in its ⋯ menu at the right end of line 2: shown
  -- on hover, on keyboard focus and on the selected row; a touch or trackpad
  -- user who never hovers finds it at the same place.
  local tools = row{ gap = 0, pin = (not menu_on_name) and { bottom = 4, right = 0 } or nil, reveal = not selected,
    menu{ "", id = "extra-sandbox-" .. name, icon = "more", variant = "ghost", dense = true, items = menu_items, on = "row_menu", arg = name, help = "Every action of " .. name },
  }

  return row{
    key = "row-" .. name, on = "open", arg = name, on_space = "toggle", selected = selected, hover_group = true,
    help = name .. ": " .. st.word .. ". Enter opens its terminal, Space starts or stops it",
    h = 52, w = "full", gap = 8, align = "stretch", radius = "sm",
    fill = selected and "elevated" or nil, hover = selected and "elevated" or "hover-subtle",
    column{ py = 8, rail(st.tone, st.shape) },
    column{ grow = 1, gap = 2, justify = "center",
      row{ w = "full", gap = 8,
        text{ name, size = "title", weight = "medium", color = "text-bright", truncate = true, grow = 1, help = name },
        menu_on_name and tools or false,
      },
      row(line2),
      (not menu_on_name) and tools or false,
    },
    column{ justify = "center", primary },
    column{ w = 0 },
  }
end

-- Features' sub-rows under a sandbox (data.app.extras.rows[name]): one per
-- session (its main agent, each thread), indented, with a small rail of
-- their own, the whole row pressable. A copy of this screen must draw these
-- too (docs/screens.md, "Extras"), or the core draws them below it.
local sub_tones = { green = "green", amber = "amber", red = "red", blue = "blue", purple = "blue" }

local function sub_rows(sb, app, into)
  for _, e in ipairs(((app.extras or {}).rows or {})[sb.name] or {}) do
    local key = "extra-row-" .. sb.name .. "-" .. e.id
    if e.intent then extra_intents[sb.name .. "\n" .. e.id] = e.intent end
    local tone = sub_tones[e.tone] or "muted"
    local label = e.label or ""
    local shape = (e.tone == "amber" and "notch") or (string.find(label, "working") and "dashed")
      or (e.tone == "red" and "broken") or ((tone == "muted") and "hollow") or "solid"
    if string.find(label, "working") then tone = "blue" end
    table.insert(into, row{ key = key, h = 28, gap = 8, w = "full", align = "stretch", radius = "sm", hover = "hover-subtle",
      on = e.intent and "extra" or nil, arg = e.intent and (sb.name .. "\n" .. e.id) or nil, help = e.help ~= "" and e.help or label,
      column{ w = 8 },
      column{ py = 4, rail(tone, shape, true) },
      row{ grow = 1, gap = 8,
        text{ label, size = "body", color = tone == "amber" and "amber" or "text", truncate = true, grow = 1 },
        e.count and badge{ "pane " .. e.count, tone = "blue" } or false,
      },
    })
  end
end

local function matches(sb, filter)
  return filter == "" or string.find(string.lower(sb.name), string.lower(filter), 1, true) ~= nil
end

-- The create feature's "New" (a top bar extra), for the empty list's action.
local function new_extra(app)
  for _, e in ipairs((app.extras or {}).topbar or {}) do
    if e.id == "create" and e.intent then return e end
  end
end

return {
  init = function()
    return { filter = "", collapsed = {} }
  end,

  render = function(state, data)
    local app = data.app
    local collapsed = state.collapsed or {}
    local colors = app.groups or {}
    local shown = {}
    for _, sb in ipairs(data.sandboxes) do
      if matches(sb, state.filter or "") then table.insert(shown, sb) end
    end
    local body = { gap = 2, group = "list", w = "full" }
    for _, g in ipairs(groups(shown)) do
      local gk = g.label or ""
      if g.label then
        local closed = collapsed[gk] == true
        local waiting = 0
        for _, sb in ipairs(g.items) do
          if state_of(sb, data).tone == "amber" then waiting = waiting + 1 end
        end
        table.insert(body, row{ key = "group-" .. gk, on = "toggle_group", arg = gk, h = 28, gap = 8, w = "full", px = 4, radius = "sm",
          hover = "hover-subtle", help = (closed and "Show " or "Hide ") .. (g.help or g.label),
          icon{ closed and "chevron-right" or "chevron-down", size = 12, color = "text-muted" },
          colors[g.color] and column{ w = 8, h = 8, radius = 2, fill = colors[g.color] } or false,
          text{ g.label, size = "caption", weight = "semibold", color = "text-muted", truncate = true },
          text{ tostring(#g.items), size = "caption", color = "text-muted" },
          spacer{},
          (closed and waiting > 0) and badge{ waiting .. " want" .. (waiting == 1 and "s" or "") .. " you", tone = "amber" } or false,
        })
        if not closed then
          for _, sb in ipairs(g.items) do
            table.insert(body, item(sb, app, data))
            sub_rows(sb, app, body)
          end
        end
      else
        if #body > 0 then table.insert(body, column{ h = 8 }) end
        for _, sb in ipairs(g.items) do
          table.insert(body, item(sb, app, data))
          sub_rows(sb, app, body)
        end
      end
    end

    -- Empty and failed states: one sentence, one action.
    local empty = false
    if #data.sandboxes == 0 then
      if app.connection.state == "up" then
        local new = new_extra(app)
        new_intent = new and new.intent or nil
        empty = column{ gap = 12, py = 16, px = 4,
          text{ "No sandboxes yet.", size = "body", weight = "medium", color = "text-bright" },
          text{ "Create one to run an agent in it.", size = "caption", color = "text-muted", wrap = true },
          new and button{ "New sandbox", icon = "plus", variant = "primary", on = "new", key = "empty-new" } or false,
        }
      else
        empty = column{ gap = 8, py = 16, px = 4,
          text{ "Waiting for the daemon.", size = "body", weight = "medium", color = "text-bright" },
          text{ "Start it with sbxw up: the list fills in as soon as it answers.", size = "caption", color = "text-muted", wrap = true },
        }
      end
    elseif #shown == 0 then
      empty = column{ gap = 12, py = 16, px = 4,
        text{ "No sandbox matches “" .. state.filter .. "”.", size = "body", color = "text-bright", wrap = true },
        button{ "Clear the filter", icon = "close", on = "clear_filter", key = "filter-clear" },
      }
    end

    return column{ gap = 12, h = "full", w = "full",
      row{ gap = 8, w = "full", h = 28, px = 4,
        text{ "Sandboxes", size = "title", weight = "semibold", color = "text-bright" },
        text{ tostring(#data.sandboxes), size = "caption", color = "text-muted" },
      },
      app.connection.state ~= "up" and row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "amber-tint",
        icon{ "warning", size = 14, color = "amber" },
        text{ app.connection.text, size = "caption", color = "text-bright", wrap = true, grow = 1 },
      } or false,
      input{ id = "filter", value = state.filter, placeholder = "Filter by name", on = "filter" },
      scroll{ gap = 0, column(body), empty },
    }
  end,

  actions = {
    filter = function(state, _, text) state.filter = text or "" end,
    clear_filter = function(state) state.filter = "" end,
    toggle_group = function(state, key)
      state.collapsed = state.collapsed or {}
      state.collapsed[key] = not state.collapsed[key] or nil
    end,
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
    -- Space on a row: stop a running sandbox, start a stopped one.
    toggle = function(_, name)
      if statuses[name] == "running" then return intent("stop_sandbox", { sandbox = name }) end
      if statuses[name] == "stopped" then return intent("start_sandbox", { sandbox = name }) end
    end,
    extra = function(_, key)
      local i = extra_intents[key]
      if i then return intent(i.name, i.args) end
    end,
    row_menu = function(_, name, id)
      if id == "bash" then return intent("open_terminal", { sandbox = name, mode = "bash", pane = "spare" }) end
      if id == "spare" then return { intent("select", { sandbox = name }), intent("open_terminal", { sandbox = name, pane = "spare" }) } end
      if id == "start" then return intent("start_sandbox", { sandbox = name }) end
      if id == "stop" then return intent("stop_sandbox", { sandbox = name }) end
      local i = extra_intents[name .. "\n" .. id]
      if i then return intent(i.name, i.args) end
    end,
    new = function()
      if new_intent then return intent(new_intent.name, new_intent.args) end
    end,
  },
}
