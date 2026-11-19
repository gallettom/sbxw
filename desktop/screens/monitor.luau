-- Monitor: every Claude session in one view, who needs you first (left), the
-- selected one as a short feed with its question and a composer (middle).
-- An overlay over the pane area, opened from the rail. Built by
-- src/feat/monitor/: `data.monitor` is its view, kept from the daemon's
-- `session` events one card at a time.
--
-- Agent text (prompts, replies, questions, options) is data: drawn as text
-- and Markdown with keyword pills, never as a control. An option's press
-- sends its index; typed words go through the core's dialog.

local TONE = { you = "amber", working = "blue", idle = "neutral", ended = "neutral" }

-- Rows already built, by key, with what they were built from: an event about
-- one session builds that one row again, not the list (`rev` moves only
-- when its report changed).
local memo = {}
local built = 0

local function ago(ms, now)
  local m = math.floor(math.max(0, (now or 0) - (ms or now or 0)) / 60000)
  if m < 1 then return "now" end
  if m < 60 then return m .. " min" end
  if m < 1440 then return math.floor(m / 60) .. " h" end
  return math.floor(m / 1440) .. " d"
end

local function square(data, sb)
  local c = ((data.app or {}).groups or {})[sb]
  return c and column{ w = 8, h = 8, radius = 2, fill = c } or column{ w = 8, h = 8, radius = 2, border = "text-dim" }
end

local function list_row(r, sel, now, color)
  local when = ago(r.ts, now)
  local sig = tostring(r.rev) .. "|" .. when .. "|" .. tostring(sel) .. "|" .. tostring(color)
  local m = memo[r.key]
  if m and m.sig == sig then return m.el end
  built = built + 1
  local el = row{ key = "monitor-row-" .. r.key, on = "pick", arg = r.key, selected = sel,
    fill = sel and "active-bg" or nil, hover = "hover-subtle", radius = "sm", px = 8, py = 8, gap = 8, w = "full", align = "start",
    column{ py = 4, dot{ tone = r.tone } },
    column{ grow = 1, gap = 2,
      row{ gap = 4, w = "full",
        color and column{ w = 8, h = 8, radius = 2, fill = color } or false,
        text{ r.sandbox, weight = "medium", color = "text-bright", truncate = true },
        text{ r.name, size = "sm", color = r.thread and "text-muted" or "text-dim", truncate = true, grow = 1 },
        text{ when, size = "sm", color = "text-dim" } },
      r.ask and text{ r.ask, size = "sm", weight = "medium", color = "amber" } or false,
      text{ r.line, size = "sm", color = "text-muted", truncate = true, mono = r.mono, pills = r.sandbox } } }
  memo[r.key] = { sig = sig, el = el }
  return el
end

local function heading(t, n)
  return row{ px = 8, py = 4, gap = 4, text{ t, size = "sm", weight = "semibold", color = "text-muted" }, text{ tostring(n), size = "sm", color = "text-dim" } }
end

local function bubble(label, mark, ic, right, body)
  return column{ gap = 4, w = "full",
    row{ gap = 4, w = "full", icon{ mark, size = 12, color = ic }, text{ label, size = "sm", weight = "medium", color = "text-muted" }, right or false },
    body }
end

local function question_card(d, data)
  local q = d.question
  local picks = (((data.relay_ui or {}).picks or {})[d.session]) or {}
  local sending = ((data.relay_ui or {}).sending or {})[d.session]
  local kids = { gap = 12, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "monitor-question",
    row{ gap = 8, w = "full", icon{ "ask", color = "amber" }, text{ "Asks you", weight = "semibold", color = "amber" }, spacer{},
      #q.steps > 1 and text{ #q.steps .. " questions", size = "sm", color = "text-muted" } or false } }
  for si, st in ipairs(q.steps) do
    local qk = { gap = 8, w = "full", text{ st.text, wrap = true, color = "text-bright", pills = d.sandbox } }
    for i, o in ipairs(st.options) do
      local picked = picks[si] == i
      qk[#qk + 1] = row{ gap = 8, w = "full", align = "start",
        q.answerable and button{ o.label, key = "monitor-opt-" .. si .. "-" .. i, on = "choose", arg = { key = d.key, step = si, option = i },
          tone = "amber", variant = picked and "primary" or nil, icon = picked and "check" or nil, disabled = sending ~= nil,
          help = "Answers with this option, the Requests panel's way" }
          or text{ i .. ". " .. o.label, color = "text", wrap = true },
        o.desc and text{ o.desc, size = "sm", wrap = true, color = "text-muted", grow = 1 } or false }
    end
    kids[#kids + 1] = column(qk)
  end
  if sending then
    kids[#kids + 1] = text{ "Sending: " .. sending, size = "sm", color = "text-muted", wrap = true }
  elseif q.answerable then
    kids[#kids + 1] = text{ #q.steps > 1 and "It is sent once each question has a pick." or "A press answers it.", size = "sm", color = "text-dim" }
  else
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ "sbxw cannot answer this session from here.", size = "sm", color = "text-muted", grow = 1, wrap = true },
      button{ "Answer in its terminal", key = "monitor-answer-terminal", icon = "terminal", tone = "green", on = "link", arg = { name = "monitor_terminal", key = d.key } } }
  end
  return column(kids)
end

local function permission_card(d)
  return column{ gap = 8, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "monitor-permission",
    row{ gap = 8, icon{ "warning", color = "amber" }, text{ "Wants your permission", weight = "semibold", color = "amber" } },
    text{ d.permission, wrap = true, color = "text-bright" },
    text{ "sbxw does not see the command it asks about. Read it, and answer, in its terminal.", size = "sm", wrap = true, color = "text-muted" },
    row{ button{ "Open its terminal", key = "monitor-permission-terminal", icon = "terminal", tone = "green", on = "link", arg = { name = "monitor_terminal", key = d.key } } } }
end

local function feed(d, now, data)
  local kids = { gap = 16, px = 24, py = 16, w = "full" }
  if d.last_input then
    kids[#kids + 1] = bubble("You", "prompt", "text-muted", nil,
      column{ pad = 12, radius = "md", fill = "elevated", w = "full", text{ d.last_input, wrap = true, color = "text-bright" } })
  end
  if d.now then
    kids[#kids + 1] = row{ gap = 8, px = 12, py = 8, radius = "sm", border = "elevated", w = "full", key = "monitor-now",
      icon{ "sparkle", size = 12, color = "blue" }, text{ "Now", size = "sm", weight = "medium", color = "blue" },
      text{ d.now, mono = true, size = "sm", color = "text", truncate = true, grow = 1 }, text{ ago(d.ts, now), size = "sm", color = "text-dim" } }
  end
  if d.reply then
    kids[#kids + 1] = bubble("Claude", "sparkle", "vendor-anthropic", text{ "· last reply, " .. ago(d.ts, now), size = "sm", color = "text-dim" },
      column{ w = "full", max_w = 720, key = "monitor-reply", markdown{ d.reply, pills = d.sandbox } })
  end
  if d.question then
    kids[#kids + 1] = question_card(d, data)
  elseif d.permission then
    kids[#kids + 1] = permission_card(d)
  end
  if not d.last_input and not d.now and not d.reply and not d.question and not d.permission then
    kids[#kids + 1] = text{ "Nothing to read yet: the session has not reported a prompt or a reply.", color = "text-muted", wrap = true }
  end
  kids[#kids + 1] = text{ "The last prompt, what it does now and its last reply: sbxw keeps no history of the conversation.", size = "sm", color = "text-dim", wrap = true }
  return scroll{ grow = 1, fade = 12, column(kids) }
end

local function draft_of(state, d)
  local dr = (state.drafts or {})[d.key]
  -- A draft typed before the last send went with it.
  if dr and dr.gen == d.sent then return dr.text end
  return ""
end

local function composer(state, d, data)
  local c = d.compose
  local covered = (data.app or {}).covered == true
  local can = c.can and not covered
  local draft = draft_of(state, d)
  return column{ gap = 8, px = 24, py = 12, w = "full",
    textarea{ id = "monitor-compose", rows = 2, value = draft, disabled = not c.can,
      placeholder = c.can and ("Write to " .. c.target .. "…") or "",
      on = "typed", on_submit = "submit", arg = { key = d.key, gen = d.sent } },
    row{ gap = 8, w = "full",
      icon{ "send", size = 12, color = "text-dim" },
      text{ "To", size = "sm", color = "text-dim" }, square(data, d.sandbox),
      text{ c.target, size = "sm", weight = "medium", color = "text", truncate = true, grow = 1, key = "monitor-target" },
      button{ "Send…", key = "monitor-send", icon = "send", tone = "green", on = "send", arg = { key = d.key, gen = d.sent },
        disabled = not can or draft == "",
        help = "Opens the core's dialog with your words in full and the session they go to", keys = { (data.app or {}).platform == "macos" and "⌘" or "Ctrl", "↩" } } },
    (not c.can) and text{ c.why, size = "sm", color = "text-dim", wrap = true } or false }
end

local function header(d, data)
  return column{ px = 24, py = 8, gap = 4, w = "full",
    row{ gap = 8, w = "full",
      dot{ tone = d.tone }, square(data, d.sandbox),
      text{ d.sandbox, size = "title", weight = "semibold", color = "text-bright", truncate = true },
      text{ d.name, color = "text-muted" },
      badge{ d.word, tone = d.tone },
      spacer{},
      d.model and text{ d.model, size = "sm", color = "vendor-anthropic" } or false,
      text{ d.cost or "no cost yet", size = "sm", color = "text-muted" } },
    row{ gap = 4, w = "full",
      text{ d.strip, size = "sm", color = "text-dim", truncate = true, grow = 1 },
      button{ icon = "terminal", key = "monitor-terminal", tone = "green", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_terminal", key = d.key },
        help = d.pane and ("Its terminal, in pane " .. d.pane .. " (the view closes)") or "Open its terminal (the view closes)", keys = { "↩" } },
      button{ icon = "history", key = "monitor-commits", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_commits", key = d.key }, help = "Its commits" },
      button{ icon = "folder", key = "monitor-files", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_files", key = d.key }, help = "Its files" },
      button{ icon = "branch", key = "monitor-worktrees", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_worktrees", key = d.key }, help = "Its worktree" } } }
end

local function empty(view)
  return column{ key = "monitor-empty", gap = 8, pad = 24, w = "full", h = "full",
    text{ view and "No session yet" or "Loading the sessions", size = "title", weight = "semibold", color = "text-bright" },
    text{ view and "A Claude session that starts in a sandbox shows here: who needs you first, then who works." or "", color = "text-muted", wrap = true },
    (not view) and row{ button{ "Show the sessions", key = "monitor-load", on = "load", tone = "blue" } } or false }
end

local screen = {
  init = function() return { drafts = {} } end,
  render = function(state, data)
    local v = data.monitor
    if not v or #(v.rows or {}) == 0 then
      memo = {}
      return empty(v)
    end
    local now, counts, d = v.now, v.counts or {}, v.detail
    local groups = (data.app or {}).groups or {}
    local rows = { gap = 2, px = 4, py = 4, w = "full", group = "list", key = "monitor-list" }
    local last
    local seen = {}
    for _, r in ipairs(v.rows) do
      if r.group ~= last then
        local title = r.group == "you" and "Needs you" or r.group == "working" and "Working" or r.group == "idle" and "Idle" or "Ended"
        rows[#rows + 1] = heading(title, counts[r.group] or 0)
        last = r.group
      end
      rows[#rows + 1] = list_row(r, r.key == v.sel, now, groups[r.sandbox])
      seen[r.key] = true
    end
    for k in pairs(memo) do if not seen[k] then memo[k] = nil end end
    local more = more_rows(v.count, { key = "monitor-more", on = "more", what = "sessions" })
    if more then rows[#rows + 1] = more end
    local you = counts.you or 0
    local top = row{ px = 12, py = 8, gap = 8, w = "full",
      text{ "Sessions", weight = "semibold", color = "text-bright" },
      text{ tostring(counts.total or 0), size = "sm", color = "text-dim" }, spacer{},
      you > 0 and button{ you .. " need" .. (you == 1 and "s" or "") .. " you", key = "monitor-next", tone = "amber", dense = true, on = "next",
        help = "The next session that needs you", keys = { "N" } }
        or text{ "Nothing needs you", key = "monitor-calm", size = "sm", color = "text-dim" } }
    local left = column{ gap = 0, w = "full", h = "full", top, scroll{ grow = 1, column(rows) } }
    local middle = d and column{ h = "full", grow = 1, key = "monitor-detail",
      header(d, data), divider{}, feed(d, now, data), divider{}, composer(state, d, data) }
    if not overlay_wide(data) or not d then
      -- Narrow: the list, or the session a press opened, with the way back.
      if d and state.open == d.key then
        return column{ h = "full", w = "full",
          row{ px = 12, py = 4, w = "full", button{ "All sessions", key = "monitor-back", icon = "threads", variant = "ghost", dense = true, on = "back" } },
          middle }
      end
      return column{ h = "full", w = "full", left }
    end
    local wide = ((data.app or {}).overlay or {}).w or 1000
    return split{ id = "monitor-split", vertical = true, ratio = wide >= 1100 and 0.3 or 0.38, left, middle }
  end,
  actions = {
    load = function() return intent("monitor_open", {}) end,
    pick = function(state, key)
      state.open = key
      return intent("monitor_select", { key = key })
    end,
    back = function(state) state.open = nil end,
    next = function() return intent("monitor_next", {}) end,
    more = function() return intent("monitor_more", {}) end,
    link = function(_, a) return intent(a.name, { key = a.key }) end,
    choose = function(_, a) return intent("monitor_pick", { key = a.key, step = a.step, option = a.option }) end,
    -- Typing sends nothing: the draft is kept, by session.
    typed = function(state, a, text)
      state.drafts = state.drafts or {}
      state.drafts[a.key] = { text = text or "", gen = a.gen }
    end,
    send = function(state, a)
      local dr = (state.drafts or {})[a.key]
      if not dr or dr.gen ~= a.gen or not string.find(dr.text, "%S") then return nil end
      return intent("monitor_send", { key = a.key, text = dr.text })
    end,
    submit = function(state, a, text)
      state.drafts = state.drafts or {}
      state.drafts[a.key] = { text = text or "", gen = a.gen }
      if not string.find(text or "", "%S") then return nil end
      return intent("monitor_send", { key = a.key, text = text })
    end,
  },
}

-- What the tests read: rows built since the screen loaded.
screen.built = function() return built end

return screen
