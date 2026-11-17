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
-- Line 1: the name (it truncates; nothing is drawn over it), the network
-- count as a bubble (blue: hosts sbx refused; amber: sbx holds something for
-- you), the pane chip (P1), the ⋯ menu (shown on hover, keyboard focus and
-- the selected row; it keeps its room). Line 2: the state word (or, when a
-- request wants you, its own words: "asks you 1"), the model the session
-- runs ("opus 5.5"; the agent's name when none is reported), the text badges
-- that fit whole, and two buttons one text line tall: + (the agent's terminal
-- in a new pane; a stopped sandbox starts) and start or stop.
--
-- Keys: row-<name>, start-<name>, stop-<name>, add-<name>, model-<name>,
-- pane-of-<name>; the row's menu extra-sandbox-<name> (its list
-- extra-sandbox-<name>-list), whose items are keyed as the inline extras
-- were: extra-sandbox-<name>-<id> (network, threads, more, ...), plus
-- extra-sandbox-<name>-bash (open a shell), -spare (open in a new pane),
-- -start, -stop; a badge or the bubble keeps its extra's key when it shows
-- inline; more-<name> is the "+N" of badges that did not fit.
-- Sessions: sessions-<name> (the disclosure line), sessions-block-<name>,
-- extra-row-<name>-<id> (a line). Groups: block-<label> (the tinted block),
-- group-<label> (its header); block-ungrouped (the rest, on the plain ground).
-- What opens and shuts (a group, a sandbox's sessions) folds: its height
-- glides over 0.18 s and its lines fade (docs/screens.md, `folds`).
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
  if e.id == "relay" then return "asks you " .. (n or "") end
  if e.id == "threads" then return n and (n .. (n == 1 and " thread" or " threads")) or "threads" end
  if e.id == "kit-add" and n then return n .. (n == 1 and " kit waits" or " kits wait") end
  local label = e.label or ""
  if n then label = (label ~= "" and (label .. " ") or "") .. tostring(n) end
  return label
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

local word_color = { amber = "amber", blue = "blue", red = "red-text", green = "text-muted", muted = "text-muted" }

-- The model a session runs, short and lower case like the state word beside
-- it: "opus 5.5" from `model_name` "Opus 5.5" or from the id
-- "claude-opus-5-5" (vendor and "claude-" dropped, the family, then the
-- version's numbers joined by dots; a date suffix dropped). Another vendor's
-- id is shown as it is, less its vendor prefix. Nil when nothing is known.
local function short_model(id)
  if not id or id == "" then return nil end
  local m = string.lower(id)
  m = m:gsub("%[.-%]$", ""):gsub("^.*/", ""):gsub("^[%a]+%.anthropic%.", ""):gsub("^anthropic%.", "")
  m = m:gsub("%-v%d+:%d+$", ""):gsub("%-%d%d%d%d%d%d%d%d$", "")
  if string.sub(m, 1, 7) == "claude-" then
    local family, nums = nil, {}
    for part in string.gmatch(string.sub(m, 8), "[^%-]+") do
      if string.match(part, "^%d+$") then table.insert(nums, part)
      elseif not family then family = part end
    end
    if family then
      return #nums > 0 and (family .. " " .. table.concat(nums, ".")) or family
    end
  end
  return m
end

-- What the row says runs in the sandbox: its model (the sandbox's main
-- session's, from GET /api/sandboxes or the session), else the agent's name.
-- Returns the words and the tooltip (the full id).
local function model_of(sb, data)
  local name, id = sb.model_name, sb.model
  if not name and not id then
    for _, s in ipairs(data.sessions or {}) do
      if s.sandbox == sb.name and not s.thread then name, id = s.model_name, s.model end
    end
  end
  local words = (name and name ~= "" and string.lower(name)) or short_model(id)
  if not words then return sb.agent or "", (sb.agent or "") .. " (model not reported)" end
  return words, (sb.agent or "agent") .. ": " .. (id or name)
end

-- The sandbox's network count as a bubble on the name's line: blue while
-- sbx only reports hosts it refused, amber when it holds something until
-- the person decides. Its tooltip says which.
local function bubble_help(name, e)
  local n = e.count or 0
  local what = n == 1 and "1 network request" or (n .. " network requests")
  if e.tone == "amber" then
    return what .. " for " .. name .. ": sbx holds something until you decide. Opens the Network panel"
  end
  return what .. " sbx refused for " .. name .. " (nothing waits on them). Opens the Network panel"
end

-- Short menu labels, verb first; the feature's long sentence is the tooltip.
local SHORT = { approvals = "Network requests", network = "Network", relay = "Requests",
  more = "Manage…", threads = "Threads and plans", ["kit-add"] = "Approve kits", codemap = "Code map" }

-- An extra's name in the row's menu: a short label, its count in brackets.
local function menu_label(e)
  local l = SHORT[e.id]
  if not l then
    l = e.label or ""
    if l == "" or utf8.len(l) == 1 or l == "⑂" then
      l = string.match(e.help or "", "^([^:]+)") or e.id
    end
  end
  if e.count then l = l .. " (" .. e.count .. ")" end
  return l
end

local function item(sb, app, data)
  local name = sb.name
  local selected = app.selected == name
  local pane = in_pane(app, name)
  local running = sb.status == "running"
  local busy = sb.status ~= "running" and sb.status ~= "stopped"
  local st = state_of(sb, data)
  statuses[name] = sb.status
  local model, model_help = model_of(sb, data)

  local menu_items = {}
  if running then
    table.insert(menu_items, { id = "bash", label = "Open shell", icon = "terminal", tone = "green", help = "A bash shell in " .. name .. ", in a new pane" })
  end
  table.insert(menu_items, { id = "spare", label = "Open in new pane", icon = "plus", tone = "green", help = "The agent's terminal in a new pane" .. (running and "" or " (starts " .. name .. ")") })
  -- Line 2's text badges: what needs reading (amber first, then counts).
  -- The network count is the name line's bubble instead.
  local wants, counts, approvals = {}, {}, nil
  local go, manage = {}, {}
  for _, e in ipairs(extras(sb, app)) do
    if e.intent then extra_intents[name .. "\n" .. e.id] = e.intent end
    if e.id == "approvals" then approvals = e
    elseif e.tone == "amber" then table.insert(wants, e) elseif e.count then table.insert(counts, e) end
    if e.intent then
      -- Colour by meaning: what it goes to is blue, what waits for you amber.
      local tone = (e.tone == "amber" and e.id ~= "codemap") and "amber" or (e.id ~= "more" and "blue" or nil)
      local it = { id = e.id, label = menu_label(e), help = e.help, tone = tone, icon = e.icon or (e.id == "threads" and "chat") or (e.id == "approvals" and "flag") or (e.id == "relay" and "chat") or nil }
      if e.id == "more" then table.insert(manage, it) else table.insert(go, it) end
    end
  end
  -- Three groups, a line between: open things, go to what it has (network,
  -- requests, threads), manage it; then start or stop on its own.
  for _, group in ipairs({ go, manage }) do
    if #group > 0 then
      table.insert(menu_items, { divider = true })
      for _, it in ipairs(group) do table.insert(menu_items, it) end
    end
  end
  if running then
    table.insert(menu_items, { divider = true })
    table.insert(menu_items, { id = "stop", label = "Stop", icon = "stop", tone = "red", help = "Stop " .. name .. " (its sessions end)", keys = { "Space" } })
  elseif not busy then
    table.insert(menu_items, { divider = true })
    table.insert(menu_items, { id = "start", label = "Start", icon = "play", tone = "green", help = "Start " .. name, keys = { "Space" } })
  end

  -- Line 2: the state word and the model, never cut while they fit the
  -- line; then the badges that fit whole (the rest are "+N", whose tooltip
  -- names them); the row's two buttons at its right end.
  -- The text column: the sidebar less its padding (24), the rail's column
  -- (7) and the row's two gaps (16).
  local col_w = ((app.sidebar or {}).w or 264) - 47
  -- Under 240 pt of sidebar the + goes (it is the menu's "Open in new
  -- pane"), so the state and the model keep their room.
  local with_add = ((app.sidebar or {}).w or 264) >= 240
  local buttons_w = (busy and 0 or 28) + (with_add and not busy and 28 or 0)
  -- When what wants you is a request with words of its own ("asks you 1",
  -- "2 kits wait"), it takes the state word's place, as an amber badge that
  -- opens it: the same meaning, said precisely, in the same room.
  local lead = nil
  if st.tone == "amber" and #wants > 0 then lead = table.remove(wants, 1) end
  local first
  if lead then
    local label = badge_text(lead)
    st.word = label
    first = row{ key = "extra-sandbox-" .. name .. "-" .. lead.id, on = lead.intent and "extra" or nil, arg = name .. "\n" .. lead.id,
      help = lead.help, hover = "active-bg", radius = "xs", h = 24, badge{ label, tone = "amber" } }
  else
    first = text{ st.word, size = "caption", weight = st.tone == "amber" and "semibold" or "normal", color = word_color[st.tone] }
  end
  local room = col_w - est(st.word) - (lead and 12 or 0) - 6 - est(model) - 6 - buttons_w
  local line2 = { gap = 4, w = "full", first,
    text{ model, size = "caption", color = "text-muted", truncate = true, help = model_help, key = "model-" .. name },
  }
  local all = {}
  for _, e in ipairs(wants) do table.insert(all, e) end
  for _, e in ipairs(counts) do table.insert(all, e) end
  -- The pane it is shown in, last: the sessions block says it too.
  if pane then table.insert(all, { id = "pane", pane = pane }) end
  local rest = {}
  for i, e in ipairs(all) do
    local label = e.pane and ("P" .. e.pane) or badge_text(e)
    local w = est(label) + 12 + 6
    -- Keep room for a "+N" while something else follows.
    local tail = (i < #all) and (est("+" .. (#all - i)) + 18) or 0
    if #rest == 0 and w + tail <= room then
      room = room - w
      if e.pane then
        table.insert(line2, badge{ label, tone = "blue", help = "Shown in pane " .. e.pane, key = "pane-of-" .. name })
      else
        local tone = e.tone == "amber" and "amber" or (e.tone == "red" and "red" or "neutral")
        local key = "extra-sandbox-" .. name .. "-" .. e.id
        if e.intent then
          table.insert(line2, row{ key = key, on = "extra", arg = name .. "\n" .. e.id, help = e.help, hover = "active-bg", radius = "xs", h = 24, badge{ label, tone = tone } })
        else
          table.insert(line2, badge{ label, tone = tone, help = e.help, key = key })
        end
      end
    else
      table.insert(rest, label)
    end
  end
  -- "+N" only where it fits whole: the state word and the model come first
  -- (every one of them is in the ⋯ menu anyway).
  if #rest > 0 and est("+" .. #rest) + 18 <= room then
    table.insert(line2, badge{ "+" .. #rest, tone = #wants > 0 and "amber" or "neutral", help = table.concat(rest, ", ") .. " (all in the ⋯ menu)", key = "more-" .. name })
  end
  table.insert(line2, spacer{})

  -- The row's two buttons, one text line tall: start or stop, and "+" (the
  -- agent's terminal in a new pane; a stopped sandbox starts first).
  if with_add and not busy and sb.unreachable_since == nil then
    table.insert(line2, button{ icon = "plus", compact = true, tone = "green", on = "spare", arg = name, key = "add-" .. name, tab_stop = false,
      help = running and ("Open " .. name .. " in a new pane") or ("Start " .. name .. " and open it in a new pane") })
  end
  if running then
    table.insert(line2, button{ icon = "stop", compact = true, tone = "red", on = "stop", arg = name, help = "Stop " .. name, keys = { "Space" }, key = "stop-" .. name, tab_stop = false })
  elseif not busy then
    table.insert(line2, button{ icon = "play", compact = true, tone = "green", on = "start", arg = name, help = "Start " .. name, keys = { "Space" }, key = "start-" .. name, tab_stop = false })
  end

  -- Line 1: the name (it truncates, nothing is drawn over it), the network
  -- bubble, the pane chip, then the ⋯ menu that holds every action, shown
  -- on hover, on keyboard focus and on the selected row; it keeps its room.
  local line1 = { w = "full", gap = 4,
    text{ name, size = "title", weight = "medium", color = "text-bright", truncate = true, grow = 1, help = name },
  }
  if approvals then
    table.insert(line1, row{ key = "extra-sandbox-" .. name .. "-approvals", on = approvals.intent and "extra" or nil, arg = name .. "\napprovals",
      help = bubble_help(name, approvals), h = 24, min_w = 24, justify = "center", radius = "xs", hover = "active-bg",
      badge{ tostring(approvals.count or ""), tone = approvals.tone == "amber" and "amber" or "blue", pill = true } })
  end
  -- The ⋯ takes its room beside the name when the whole name still fits;
  -- a name too long for the line keeps all of it at rest, and the ⋯ shows
  -- over its end only on hover or focus, on the row's own fill.
  local name_w = (utf8.len(name) or #name) * 8.4 + (approvals and 28 or 0)
  local fits = name_w + 28 <= col_w
  table.insert(line1, row{ gap = 0, reveal = not selected,
    pin = (not fits) and { right = 0, top = 0 } or nil, fill = (not fits) and (selected and "elevated" or "hover-subtle") or nil, radius = "xs",
    menu{ "", id = "extra-sandbox-" .. name, icon = "more", variant = "ghost", dense = true, items = menu_items, on = "row_menu", arg = name, help = "Every action of " .. name },
  })

  return row{
    key = "row-" .. name, on = "open", arg = name, on_space = "toggle", selected = selected, hover_group = true,
    help = name .. ": " .. st.word .. ", " .. model,
    shortcuts = { { "Open its terminal", "Enter" }, { "Start or stop", "Space" } },
    h = 52, w = "full", gap = 8, align = "stretch", radius = "sm",
    fill = selected and "elevated" or nil, hover = selected and "elevated" or "hover-subtle",
    column{ py = 8, w = 7, align = "start", rail(st.tone, st.shape) },
    column{ grow = 1, gap = 0, justify = "center",
      row(line1),
      row(line2),
    },
    column{ w = 0 },
  }
end

-- Features' rows under a sandbox (data.app.extras.rows[name]): one per
-- session (its main agent, each thread). The list draws them as the web UI's
-- "Sessions" block, improved: a disclosure line ("4 sessions · 1 wants you",
-- key sessions-<name>) that folds open a block grouped by where each session
-- works: "Main checkout" (main, and threads in the same checkout), then "Own
-- worktree" (a thread each). A line: its mark (the small rail; a check when
-- done), its name, its state word, its model when it differs from the
-- sandbox's, the pane it is in (P1). Pressing a line runs its extra (opens or
-- shows its terminal). Shut by default, except for the selected sandbox and
-- one with a session that wants you; the person's choice is kept per sandbox.
local sub_tones = { green = "green", amber = "amber", red = "red", blue = "blue", purple = "blue" }

local function parse_row(e)
  local label = e.label or ""
  -- The threads feature marks the main checkout ✦ and a worktree ⑂ (3 bytes each).
  local glyph = string.sub(label, 1, 3)
  local own = glyph == "⑂"
  local rest = (glyph == "⑂" or glyph == "✦") and string.gsub(string.sub(label, 4), "^%s+", "") or label
  local name, word = string.match(rest, "^(.-) · (.*)$")
  return own, name or rest, word or ""
end

local function thread_model(data, sb, thread)
  local v = ((data.threads or {})[sb] or {}).threads or {}
  for _, t in ipairs(v) do
    if t.name == thread then return t.model end
  end
  for _, s in ipairs(data.sessions or {}) do
    if s.sandbox == sb and s.thread == thread then return s.model_name or s.model end
  end
end

local function session_line(sb, e, data, base_model)
  local key = "extra-row-" .. sb.name .. "-" .. e.id
  if e.intent then extra_intents[sb.name .. "\n" .. e.id] = e.intent end
  local _, name, word = parse_row(e)
  local tone = sub_tones[e.tone] or "muted"
  local shape = (e.tone == "amber" and "notch") or (word == "working" and "dashed") or (word == "starting" and "dashed")
    or (e.tone == "red" and "broken") or ((tone == "muted") and "hollow") or "solid"
  if word == "working" then tone = "blue" end
  local done = word == "done"
  local m = e.id ~= "main" and thread_model(data, sb.name, e.id) or nil
  local mw = m and (string.find(m, " ") and string.lower(m) or short_model(m)) or nil
  if mw == base_model then mw = nil end
  -- The name first; the model only where it fits beside the name and word.
  local room = ((data.app.sidebar or {}).w or 264) - 24 - 8 - 32 - 16 - (e.count and 32 or 0)
  if mw and est(name) + est(word) + est(mw) + 16 > room then mw = nil end
  local mark = done and icon{ "check", size = 12, color = "text-muted" } or rail(tone, shape, true)
  local wants = e.tone == "amber"
  return row{ key = key, h = 28, gap = 8, w = "full", align = "stretch", radius = "sm", hover = "hover-subtle",
    on = e.intent and "extra" or nil, arg = e.intent and (sb.name .. "\n" .. e.id) or nil,
    help = name .. (word ~= "" and (": " .. word) or "") .. (m and (", " .. m) or "") .. (e.count and (". Shown in pane " .. e.count) or ""),
    column{ w = 12 },
    column{ py = done and 8 or 4, w = 12, align = done and "center" or "start", mark },
    row{ grow = 1, gap = 8,
      text{ name, size = "body", weight = wants and "medium" or "normal", color = done and "text-muted" or (wants and "amber" or "text"), truncate = true },
      word ~= "" and text{ word, size = "caption", weight = wants and "semibold" or "normal", color = done and "text-muted" or (word_color[tone] or "text-muted") } or false,
      mw and text{ mw, size = "caption", color = "text-muted", truncate = true } or false,
      spacer{},
      e.count and badge{ "P" .. e.count, tone = "blue", help = "Shown in pane " .. e.count .. " (" .. (data.app.platform == "macos" and "⌘" or "Ctrl+") .. e.count .. " focuses it)" } or false,
    },
  }
end

-- The sessions of one sandbox: the disclosure line, and the block when open.
local function sessions_block(sb, app, data, state, into, base_model)
  local rows = ((app.extras or {}).rows or {})[sb.name] or {}
  if #rows == 0 then return end
  local waiting, done = 0, 0
  for _, e in ipairs(rows) do
    if e.tone == "amber" then waiting = waiting + 1 end
    local _, _, w = parse_row(e)
    if w == "done" then done = done + 1 end
  end
  local open = (state.sessions or {})[sb.name]
  if open == nil then open = app.selected == sb.name or waiting > 0 end
  table.insert(into, row{ key = "sessions-" .. sb.name, on = "toggle_sessions", arg = { name = sb.name, open = open }, h = 24, gap = 4, w = "full", radius = "xs",
    hover = "hover-subtle", help = (open and "Hide" or "Show") .. " the sessions of " .. sb.name,
    column{ w = 12 },
    icon{ open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
    text{ #rows .. " sessions", size = "caption", color = "text-muted" },
    waiting > 0 and text{ "· " .. waiting .. (waiting == 1 and " wants you" or " want you"), size = "caption", weight = "semibold", color = "amber" } or false,
    (not open and waiting == 0 and done > 0) and text{ "· " .. done .. " done", size = "caption", color = "text-muted" } or false,
  })
  -- The block stays in the tree with its height set: shut it is 0 tall,
  -- open as tall as its lines, so the height is what changes (on the
  -- layout spring, in answer to the press) and it clips what it gains or
  -- loses while they fade.
  local block = { key = "sessions-block-" .. sb.name, gap = 0, w = "full", clip = true, h = 0 }
  if open then
    local main, own = {}, {}
    for _, e in ipairs(rows) do
      local is_own = parse_row(e)
      table.insert(is_own and own or main, e)
    end
    local h = 0
    for _, g in ipairs({ { "Main checkout", main }, { "Own worktree", own } }) do
      if #g[2] > 0 then
        table.insert(block, row{ h = 20, w = "full", column{ w = 36 }, text{ g[1], size = "caption", color = "text-muted" } })
        h = h + 20
        for _, e in ipairs(g[2]) do
          table.insert(block, session_line(sb, e, data, base_model))
          h = h + 28
        end
      end
    end
    block.h = h + 4
  end
  table.insert(into, column(block))
end

-- `#rrggbb` drawn at `a` over `under` (the group tints).
local function mix(fg, a, under)
  local function ch(h, i) return tonumber(string.sub(h, i, i + 1), 16) end
  local out = "#"
  for _, i in ipairs({ 2, 4, 6 }) do
    out = out .. string.format("%02x", math.floor(ch(fg, i) * a + ch(under, i) * (1 - a) + 0.5))
  end
  return out
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
    return { filter = "", collapsed = {}, sessions = {} }
  end,

  render = function(state, data)
    local app = data.app
    local collapsed = state.collapsed or {}
    local colors = app.groups or {}
    local shown = {}
    for _, sb in ipairs(data.sandboxes) do
      if matches(sb, state.filter or "") then table.insert(shown, sb) end
    end
    -- One block per sandbox (its row, its sessions), one tinted block per
    -- group (the group's colour at 12% on the sidebar, as the web UI's 15%
    -- over its darker floor), all in a folding column: what opens or shuts
    -- glides (docs/screens.md, `folds`).
    local body = { gap = 4, group = "list", w = "full" }
    -- A sandbox in its group's block: its row, then its sessions line and
    -- block, side by side in the block's folding stack (one level of folds:
    -- what changes height is always a row of the stack that glides it).
    local function sandbox_rows(sb, blk)
      table.insert(blk, item(sb, app, data))
      sessions_block(sb, app, data, state, blk, (model_of(sb, data)))
    end
    for _, g in ipairs(groups(shown)) do
      local gk = g.label or ""
      if g.label then
        local closed = collapsed[gk] == true
        local waiting = 0
        for _, sb in ipairs(g.items) do
          if state_of(sb, data).tone == "amber" then waiting = waiting + 1 end
        end
        local gc = colors[g.color]
        local blk = { key = "block-" .. gk, gap = 2, w = "full", radius = "md", pad = 4, folds = true, clip = true,
          -- theme.rs GROUP_TINT (0.10), GROUP_TINT_HOVER (0.16), GROUP_LABEL_LIGHT
          -- (0.40): the contrast test holds every hue at these.
          fill = gc and mix(gc, 0.10, "#161b22") or nil,
          row{ key = "group-" .. gk, on = "toggle_group", arg = gk, h = 28, gap = 8, w = "full", px = 4, radius = "sm",
            hover = gc and mix(gc, 0.16, "#161b22") or "hover-subtle", help = (closed and "Show " or "Hide ") .. (g.help or g.label),
            icon{ closed and "chevron-right" or "chevron-down", size = 12, color = "text-muted" },
            gc and column{ w = 8, h = 8, radius = 2, fill = gc } or false,
            text{ g.label, size = "caption", weight = "semibold", color = gc and mix("#ffffff", 0.40, gc) or "text-muted", truncate = true },
            text{ tostring(#g.items), size = "caption", color = gc and mix("#ffffff", 0.40, gc) or "text-muted" },
            spacer{},
            (closed and waiting > 0) and badge{ waiting .. " want" .. (waiting == 1 and "s" or "") .. " you", tone = "amber" } or false,
          },
        }
        if not closed then
          for _, sb in ipairs(g.items) do sandbox_rows(sb, blk) end
        end
        table.insert(body, column(blk))
      else
        local blk = { key = "block-ungrouped", gap = 2, w = "full", px = 4, py = 4, folds = true }
        for _, sb in ipairs(g.items) do sandbox_rows(sb, blk) end
        table.insert(body, column(blk))
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
    toggle_sessions = function(state, a)
      state.sessions = state.sessions or {}
      state.sessions[a.name] = not a.open
    end,
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
