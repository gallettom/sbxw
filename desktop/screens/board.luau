-- The Board (an overlay over the pane area, feat/board/): a team's
-- conversation, as sbxw saw it. The person's acts on the right, the lead and
-- the threads on the left, sbxw's own events in the middle; the board's
-- decisions and warnings stand out as labelled notes; a thread's commits
-- fold into one row. Beside it (760 pt and more): the team, the notes that
-- still stand, and the commits per thread with their diff in place.
--
-- A message reads as a chat: the author line above the bubble (the
-- thread's dot and name, the role when it adds something, "to" whom, the
-- time), the bubble under it holding only the content; the same author to
-- the same addressee within minutes shares one author line. A report that
-- is only a change of state is one quiet line, no bubble.
--
-- Read only. Every text here was written by an agent, a person or git: it
-- is drawn as words (the prose face, pills for agents' words), never as a
-- control. Acting happens where it always does: the links go there.

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local ICON = { green = "check", amber = "warning", red = "warning", blue = "info", purple = "sparkle", neutral = "info" }

-- Words an agent wrote: the prose face with keyword pills (the team's
-- sandbox's switch). A person's or git's words: plain.
local function words(t, sb, opts)
  opts = opts or {}
  return text{ t, wrap = true, size = opts.size or "md", color = opts.color or "text", pills = sb or true, key = opts.key }
end

local function when(r)
  return text{ r.when or "", size = "xs", color = "text-dim", help = r.exact }
end

local function fold_button(r)
  if not r.fold then return false end
  return button{ r.fold.open and "Show less" or "Show more", variant = "ghost", tone = "blue", dense = true, on = "expand", arg = r.fold.key, key = "board-fold-" .. r.fold.key }
end

-- The conversation column's width, set by render from the overlay's, and a
-- bubble's widest (about 72 characters of prose).
local CONV_W = 620
local BUBBLE_MAX = 520
local function bubble_w() return math.max(120, math.min(BUBBLE_MAX, CONV_W - 24)) end

-- A team in "All teams": its colour and its threads' names (its summary in
-- the tooltip), never a cut sentence.
local function team_tag(r)
  if not nz(r.team_label) then return false end
  return row{ gap = 4, max_w = math.max(80, math.min(240, CONV_W - 220)), help = r.team_help,
    column{ w = 8, h = 8, radius = 2, fill = r.team_color or "text-dim" },
    text{ r.team_label, size = "xs", mono = true, color = "text-muted", truncate = true },
  }
end

-- The author line, above the bubble: dot, name (mono, the thread's colour),
-- the role only when the name does not say it, "to" whom, the time. The
-- person's: on the right, "You" last.
local function author_line(r)
  local nw = math.max(60, math.min(260, CONV_W - 200))
  if r.side == "right" then
    return row{ gap = 6, w = "full", justify = "end", key = "board-author-" .. (r.id or ""),
      team_tag(r), when(r),
      nz(r.to) and text{ "to " .. r.to, size = "xs", color = "text-muted", truncate = true, max_w = nw } or false,
      text{ "You", size = "sm", weight = "semibold", color = "text-bright" } }
  end
  return row{ gap = 6, w = "full", key = "board-author-" .. (r.id or ""),
    column{ w = 8, h = 8, radius = 4, fill = r.who_color or "text-dim" },
    text{ r.who or "", size = "sm", mono = true, weight = "semibold", color = r.who_color or "text-bright", truncate = true, max_w = nw },
    nz(r.role) and badge{ r.role, tone = "purple", help = "The session that planned this team" } or false,
    nz(r.to) and text{ "to " .. r.to, size = "xs", color = "text-muted", truncate = true, max_w = nw } or false,
    r.guess and text{ "team inferred", size = "xs", color = "text-dim", help = "From before entries named their plan: placed by its time and author" } or false,
    team_tag(r), when(r) }
end

local function link(l, key)
  if not l then return false end
  return button{ l.label, variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = l.what, thread = l.thread, relay = l.relay }, key = key }
end

-- The small quiet line at a bubble's top: what kind of message it is.
local function kind_line(label, opts)
  opts = opts or {}
  return row{ gap = 4, opts.icon and icon{ opts.icon, size = 12, color = opts.icon_color or "text-muted" } or false,
    text{ label, size = "xs", color = opts.color or "text-muted", weight = opts.weight, help = opts.help },
    nz(opts.extra) and text{ opts.extra, size = "xs", color = "text-muted", help = opts.extra_help } or false }
end

-- A message: its author line (unless it follows the same author's), then
-- its bubble, hugging its content up to the bubble's widest, on its side.
local function bubble(r, kids, opts)
  opts = opts or {}
  local right = r.side == "right"
  local box = { gap = 4, px = 12, py = 8, radius = "md", max_w = bubble_w(), key = "board-bubble-" .. (r.id or ""),
    fill = opts.fill or (right and "surface-alt" or "surface"), border = opts.border or "elevated" }
  for _, k in ipairs(kids) do box[#box + 1] = k end
  return column{ gap = 2, w = "full", py = r.cont and 0 or 4, key = "board-msg-" .. (r.id or ""),
    (not r.cont) and author_line(r) or false,
    row{ w = "full", justify = right and "end" or "start", px = right and 0 or 14, column(box) } }
end

local function questions(r, sb)
  local out = { gap = 8 }
  for _, q in ipairs(r.questions or {}) do
    local kids = { gap = 2, px = 8, py = 4, radius = "xs", fill = "floor",
      text{ (q.id or "") .. (nz(q.title) and (" · " .. q.title) or ""), size = "sm", weight = "semibold", color = "text-bright", wrap = true },
      words(q.text or "", sb, { size = "sm" }),
    }
    if nz(q.recommendation) then kids[#kids + 1] = text{ "Suggested: " .. q.recommendation, size = "sm", color = "text-muted", wrap = true } end
    if nz(q.answer) then
      kids[#kids + 1] = row{ gap = 4, align = "start", icon{ "check", size = 12, color = "green" }, text{ "Answer: " .. q.answer, size = "sm", color = "text-bright", wrap = true, grow = 1, min_w = 0 } }
    end
    out[#out + 1] = column(kids)
  end
  return column(out)
end

local NOTE_LOOK = {
  board_decision = { "green-tint", "green" }, board_warning = { "amber-tint", "amber" },
  board_question = { "blue-tint", "blue" }, board_handoff = { "surface-alt", "elevated" }, conflict = { "red-tint", "red-border" },
}
-- A report's tint by its state: done green, blocked amber, failed red.
local TINT = { green = { "green-tint", "green" }, amber = { "amber-tint", "amber" }, red = { "red-tint", "red-border" } }

local function rowview(r, sb)
  if r.type == "day" then
    return row{ gap = 8, w = "full", py = 4, key = "board-" .. r.id, column{ grow = 1, h = 1, fill = "elevated" }, text{ r.label, size = "xs", color = "text-muted" }, column{ grow = 1, h = 1, fill = "elevated" } }
  elseif r.type == "system" then
    return row{ gap = 8, w = "full", justify = "center", key = "board-msg-" .. (r.id or ""),
      icon{ ICON[r.tone] or "info", size = 12, color = r.tone == "neutral" and "text-muted" or r.tone },
      text{ r.text or "", size = "sm", color = "text-muted", wrap = true, max_w = math.max(120, math.min(560, CONV_W - 200)) },
      team_tag(r), when(r), link(r.link, "board-link-" .. (r.id or "")) }
  elseif r.type == "transition" then
    -- A bare change of state: the agent, an arrow, the new state, no bubble.
    local n = r.count or 1
    return row{ gap = 6, w = "full", px = 4, key = "board-msg-" .. (r.id or ""),
      help = n > 1 and (n .. " changes of state: " .. (r.trail or "") .. " (from " .. (r.first_when or "") .. ")") or nil,
      column{ w = 8, h = 8, radius = 4, fill = r.who_color or "text-dim" },
      text{ r.who or "", size = "sm", mono = true, color = r.who_color or "text-muted", truncate = true, max_w = math.max(60, math.min(260, CONV_W - 220)) },
      icon{ "chevron-right", size = 12, color = "text-dim" },
      badge{ r.state.word, tone = r.state.tone },
      n > 1 and text{ "×" .. n, size = "xs", color = "text-muted" } or false,
      team_tag(r), when(r) }
  elseif r.type == "note" then
    local look = NOTE_LOOK[r.look or r.kind] or { "surface", "elevated" }
    local kids = {
      kind_line(r.label, { icon = ICON[r.tone] or "info", icon_color = r.tone == "neutral" and "text-muted" or r.tone, color = "text-bright", weight = "semibold",
        extra = r.note_via, extra_help = "Sent through sbxw with team_note" }),
      words(r.text or "", sb),
      fold_button(r),
    }
    local files = r.files or {}
    if #files > 0 then kids[#kids + 1] = text{ table.concat(files, ", "), size = "xs", mono = true, color = "text-muted", wrap = true } end
    return bubble(r, kids, { fill = look[1], border = look[2] })
  elseif r.type == "card" then
    local chips = { gap = 4, wrap = true }
    for _, t in ipairs(r.threads or {}) do
      chips[#chips + 1] = row{ gap = 4, px = 8, h = 24, radius = "pill", fill = "floor", column{ w = 8, h = 8, radius = 4, fill = t.color }, text{ t.name, size = "xs", mono = true, color = "text-bright" } }
    end
    local kids = {
      row{ gap = 4, badge{ "Plan", tone = "purple" }, r.variant and text{ r.variant == "shared" and "one shared worktree" or "a worktree per thread", size = "xs", color = "text-muted" } or false },
      words(r.text or "", sb, { color = "text-bright" }),
      fold_button(r),
      nz(r.why) and text{ "Why: " .. r.why, size = "sm", color = "text-muted", wrap = true } or false,
      row(chips),
      button{ r.goals.open and "Hide goals" or "Show goals", variant = "ghost", tone = "blue", dense = true, on = "expand", arg = r.goals.key, key = "board-goals-" .. r.id },
    }
    if r.goals.open then
      for _, t in ipairs(r.threads or {}) do
        kids[#kids + 1] = column{ gap = 2, text{ t.name, size = "sm", mono = true, weight = "semibold", color = t.color }, words(t.goal or "", sb, { size = "sm" }) }
      end
    end
    return bubble(r, kids, { fill = "surface-alt", border = "purple" })
  elseif r.type == "status" then
    local kids = {}
    if r.state then
      kids[#kids + 1] = row{ gap = 6, badge{ r.state.word, tone = r.state.tone }, text{ "status report", size = "xs", color = "text-muted" } }
      kids[#kids + 1] = words(r.text or "", sb, { size = "sm" })
    else
      if nz(r.label) and r.side ~= "right" then kids[#kids + 1] = kind_line(r.label, { color = r.tone == "neutral" and "text-muted" or r.tone }) end
      if nz(r.text) then kids[#kids + 1] = r.side == "right" and text{ r.text, size = "sm", wrap = true, color = "text" } or words(r.text, sb, { size = "sm" }) end
    end
    kids[#kids + 1] = fold_button(r)
    local t = TINT[r.tint or ""]
    return bubble(r, kids, t and { fill = t[1], border = t[2] } or nil)
  elseif r.type == "question" then
    local kids = { kind_line(r.label or "", { color = r.tone == "neutral" and "text-bright" or r.tone, weight = "semibold" }) }
    if nz(r.text) then kids[#kids + 1] = r.side == "right" and text{ r.text, size = "sm", wrap = true, color = "text" } or words(r.text, sb, { size = "sm" }) end
    kids[#kids + 1] = fold_button(r)
    if r.questions then kids[#kids + 1] = questions(r, sb) end
    if nz(r.hidden_text) then kids[#kids + 1] = note(r.hidden_text, "text-dim") end
    kids[#kids + 1] = link(r.link, "board-link-" .. (r.id or ""))
    return bubble(r, kids)
  elseif r.type == "message" then
    return bubble(r, {
      kind_line(r.label, { icon = "chat", help = "Sent with SendMessage between the team's sessions; sbxw shows what its hook saw" }),
      words(r.text or "", sb, { size = "sm" }),
      fold_button(r),
    })
  elseif r.type == "merge" then
    local order = {}
    for i, t in ipairs(r.order or {}) do order[#order + 1] = i .. ". " .. t end
    return bubble(r, {
      row{ gap = 4, badge{ "Merge proposed", tone = "purple" } },
      text{ table.concat(order, "   "), size = "sm", mono = true, color = "text-bright", wrap = true },
      nz(r.text) and words(r.text, sb, { size = "sm" }) or false,
      link({ label = "Plan dialog", what = "plan" }, "board-link-" .. r.id),
    }, { fill = "surface-alt", border = "purple" })
  elseif r.type == "commits" then
    local head = row{ gap = 8, w = "full", key = "board-msg-" .. r.id, px = 8, h = 28, radius = "xs",
      on = r.count > 1 and "expand" or "commit", arg = r.count > 1 and r.key or r.lines[1].hash, hover = "hover-subtle",
      help = r.count > 1 and (r.open and "Fold the commits" or "Show the commits") or "Open it in the Commits column",
      icon{ r.count > 1 and (r.open and "chevron-down" or "chevron-right") or "history", size = 12, color = "text-muted" },
      column{ w = 8, h = 8, radius = 4, fill = r.who_color },
      r.count > 1 and text{ r.text, size = "sm", color = "text-bright", truncate = true, grow = 1 }
        or row{ gap = 8, grow = 1, min_w = 0, text{ r.lines[1].short or "", size = "xs", mono = true, color = "text-muted" }, text{ r.text or "", size = "sm", color = "text-bright", truncate = true, grow = 1 } },
      text{ "+" .. tostring(r.added or 0), size = "xs", mono = true, color = "green" },
      text{ "-" .. tostring(r.removed or 0), size = "xs", mono = true, color = "red" },
      team_tag(r), when(r) }
    local kids = { gap = 2, w = "full", head }
    if r.count > 1 and r.open then
      for _, l in ipairs(r.lines) do
        kids[#kids + 1] = row{ gap = 8, w = "full", px = 24, h = 24, radius = "xs", key = "board-commit-" .. l.hash, on = "commit", arg = l.hash, hover = "hover-subtle", help = "Open it in the Commits column",
          text{ l.short or "", size = "xs", mono = true, color = "text-muted" },
          text{ l.subject or "", size = "sm", color = "text", truncate = true, grow = 1 },
          text{ "+" .. tostring(l.added or 0), size = "xs", mono = true, color = "green" },
          text{ "-" .. tostring(l.removed or 0), size = "xs", mono = true, color = "red" } }
      end
    end
    return column(kids)
  end
  return false
end

-- ── The team picker ─────────────────────────────────────────────────────
-- A team is named by its threads (mono, its colour mark), its summary under
-- it on up to two lines, its state and age at the right. The list is drawn
-- here (the core's menus are one line, 320 pt): at least as wide as the
-- button, up to 560 pt, never wider than the overlay. A press outside, a
-- pick or Escape closes it; Up, Down and Enter go through `board_key`.

local function picker_w(ow, wide)
  if wide then return math.min(420, ow - 260) end
  return math.max(140, math.min(300, ow - 220))
end

local function menu_w(ow, wide)
  return math.max(picker_w(ow, wide), math.min(560, ow - 16))
end

-- About two lines of the summary at that width (the prose face, about 6.5
-- pt a character at 12 pt), cut on a word.
local function two_lines(s, w)
  local n = math.max(24, math.floor((w - 48) / 6.5) * 2)
  if utf8.len(s) == nil or utf8.len(s) <= n then return s end
  local cut = string.sub(s, 1, (utf8.offset(s, n + 1) or (#s + 1)) - 1)
  local sp = string.find(cut, " [^ ]*$")
  if sp and sp > #cut / 2 then cut = string.sub(cut, 1, sp - 1) end
  return cut .. "…"
end

local function picker_button(ui, s, ow, wide)
  local p = ui.picker or {}
  local names, summary, color
  if s then names, summary, color = s.names, s.summary, s.color
  elseif ui.all then names, summary = "All teams", "every team's latest messages"
  else names = "Pick a team" end
  local bw = picker_w(ow, wide)
  return row{ key = "title-slot", gap = 8,
    row{ key = "board-row-picker", on = "picker", w = bw, h = 28, px = 8, gap = 8, radius = "sm", border = p.open and "accent" or "elevated", fill = "surface",
      help = (nz(summary) and summary or "Pick a team") .. " (pick another team)",
      column{ w = 8, h = 8, radius = 2, fill = color or "text-dim", key = "board-team-color" },
      text{ names or "", size = "sm", mono = true, color = "text-bright", truncate = true, max_w = math.floor(bw * 0.55) },
      nz(summary) and wide and text{ summary, size = "xs", color = "text-muted", truncate = true, grow = 1 } or spacer{},
      icon{ "chevron-down", size = 12, color = "text-muted" } },
    s and badge{ s.state or "", tone = s.tone or "neutral" } or false }
end

local function picker_row(t, i, p, mw)
  local hl = (p.hl or 0) == i
  local current = p.current == t.id
  return column{ key = "board-row-team-" .. t.id, on = "team", arg = t.id, gap = 2, px = 8, py = 6, radius = "xs", w = "full",
    fill = hl and "active-bg" or nil, hover = "hover-subtle", selected = current, help = t.summary,
    row{ gap = 8, w = "full",
      column{ w = 8, h = 8, radius = 2, fill = t.color or "text-dim" },
      text{ t.names or "", size = "sm", mono = true, weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
      current and icon{ "check", size = 12, color = "accent" } or false,
      nz(t.state) and badge{ t.state, tone = t.tone or "neutral" } or false,
      nz(t.when) and text{ t.when, size = "xs", color = "text-dim" } or false },
    nz(t.summary) and text{ two_lines(t.summary, mw), size = "xs", color = "text-muted", wrap = true } or false }
end

local function picker_menu(ui, ow, oh, wide)
  local p = ui.picker or {}
  if not p.open then return false end
  local mw = menu_w(ow, wide)
  local list = { gap = 2, w = "full", key = "board-picker-list" }
  list[#list + 1] = picker_row({ id = "all", names = "All teams", summary = "Every team's latest messages, each with its team", color = "text-dim" }, 0, p, mw)
  for i, t in ipairs(ui.teams or {}) do list[#list + 1] = picker_row(t, i, p, mw) end
  if (p.more or 0) > 0 then
    list[#list + 1] = row{ w = "full", justify = "center", button{ "Show " .. p.more .. " more", variant = "ghost", tone = "blue", dense = true, on = "picker_more", key = "board-picker-more" } }
  end
  local est = 8 + 54 * (#(ui.teams or {}) + 1) + ((p.more or 0) > 0 and 32 or 0)
  local h = math.max(80, math.min(est, (oh or 600) - 24))
  return column{ pin = { left = 0, top = 0 }, w = mw, key = "board-picker", fill = "surface", border = "elevated", radius = "md", pad = 6,
    scroll{ h = h, key = "board-picker-scroll", column(list) } }
end

local GROUPS = { { "all", "All" }, { "notes", "Decisions and warnings" }, { "questions", "Questions" }, { "messages", "Messages" }, { "commits", "Commits" }, { "status", "Status" } }
local GROUP_HELP = { messages = "What the lead and threads sent each other with SendMessage, as sbxw saw it: from the day the daemon had this version, none from before" }

local function filters(ui, wide)
  local seg = { gap = 4, w = "full", wrap = true }
  for _, g in ipairs(GROUPS) do
    local on_it = ui.group == g[1]
    seg[#seg + 1] = button{ g[2], variant = on_it and "neutral" or "ghost", color = on_it and "text-bright" or "text-muted", dense = true, on = "group", arg = g[1], key = "board-group-" .. g[1], help = GROUP_HELP[g[1]] }
  end
  local threads = { { id = "", label = "Every thread" } }
  if ui.side then
    threads[#threads + 1] = { id = "lead", label = "lead" }
    for _, t in ipairs(ui.side.threads or {}) do threads[#threads + 1] = { id = t.name, label = t.name } end
  end
  seg[#seg + 1] = not ui.all and select{ id = "board-thread", value = ui.thread or "", items = threads, on = "thread", w = 160 } or false
  seg[#seg + 1] = input{ id = "board-search", value = ui.text or "", placeholder = "Search the conversation", on = "search", grow = 1, min_w = 160 }
  return row(seg)
end

local function conversation(ui, sb)
  local list = { gap = 4, w = "full", px = 4, folds = true, key = "board-rows" }
  if ui.older then
    list[#list + 1] = row{ w = "full", justify = "center", button{ "Show earlier", variant = "ghost", tone = "blue", on = "more", key = "board-more", help = ui.hidden > 0 and (ui.hidden .. " earlier rows read") or "Read older events" } }
  end
  for _, r in ipairs(ui.rows or {}) do
    local v = rowview(r, sb)
    if v then list[#list + 1] = v end
  end
  if #(ui.rows or {}) == 0 then
    list[#list + 1] = note(ui.loading and "Reading the conversation…" or ((#(ui.teams or {}) == 0) and "No team yet: a lead's plan makes one (Team mode on a sandbox)." or (ui.group == "messages" and "No direct message. They show from the day the daemon had this version; none from before is recovered." or "Nothing here matches.")))
  end
  for _, e in ipairs(ui.errors or {}) do list[#list + 1] = note(e, "red") end
  return scroll{ key = "board-convo", stick_bottom = true, column(list) }
end

local function section(title, extra)
  return row{ gap = 8, w = "full", text{ title, size = "sm", weight = "semibold", color = "text-muted" }, extra or false }
end

local function team_view(s, ui)
  local out = { gap = 8, w = "full" }
  out[#out + 1] = section("Team", badge{ s.state or "", tone = s.tone or "neutral" })
  out[#out + 1] = row{ gap = 8, w = "full", badge{ "lead", tone = "purple" }, text{ s.lead or "the sandbox's main session", size = "sm", color = "text-bright", truncate = true, grow = 1 } }
  for _, t in ipairs(s.threads or {}) do
    out[#out + 1] = row{ gap = 8, w = "full", px = 4, py = 4, radius = "xs", key = "board-thread-" .. t.name, on = "thread", arg = t.selected and "" or t.name,
      selected = t.selected, fill = t.selected and "active-bg" or nil, hover_group = true,
      help = t.selected and "Every thread again" or ("Only " .. t.name .. " in the conversation"),
      dot{ tone = t.tone },
      column{ gap = 2, grow = 1, min_w = 0,
        row{ gap = 8, text{ t.name, size = "sm", weight = "semibold", color = t.color, truncate = true }, text{ t.state, size = "xs", color = t.tone == "neutral" and "text-muted" or t.tone } },
        nz(t.meta) and text{ t.meta, size = "xs", color = "text-muted", truncate = true } or false },
      button{ icon = "terminal", variant = "ghost", dense = true, on = "open", arg = { what = "terminal", thread = t.name }, key = "board-term-" .. t.name, help = "Its terminal" },
      ui.monitor and button{ icon = "inbox", variant = "ghost", dense = true, on = "open", arg = { what = "monitor", thread = t.name }, key = "board-mon-" .. t.name, help = "The Monitor on it" } or false,
    }
  end
  out[#out + 1] = row{ gap = 4, w = "full", wrap = true,
    button{ "Plan dialog", variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = "plan" }, key = "board-plan", help = "Approve, follow or merge the plan there" },
    button{ "Threads", variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = "threads" }, key = "board-threads", help = "The Threads panel of " .. (s.sandbox or "") },
    button{ "Worktrees", variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = "worktrees" }, key = "board-worktrees" },
  }
  return column(out)
end

local function notes_view(s, sb)
  local out = { gap = 8, w = "full" }
  out[#out + 1] = section("Decisions and warnings", #(s.notes or {}) > 0 and badge{ tostring(#s.notes), tone = "neutral" } or false)
  if #(s.notes or {}) == 0 then out[#out + 1] = note("None on the board.") end
  for _, n in ipairs(s.notes or {}) do
    out[#out + 1] = column{ gap = 2, w = "full", px = 8, py = 4, radius = "sm", key = "board-pin-" .. n.id,
      fill = n.tone == "amber" and "amber-tint" or "green-tint", border = n.tone,
      row{ gap = 4, icon{ n.tone == "amber" and "warning" or "check", size = 12, color = n.tone }, text{ n.label, size = "xs", weight = "semibold", color = "text-bright" }, text{ n.who or "", size = "xs", color = n.who_color } },
      words(n.text or "", sb, { size = "sm" }) }
  end
  return column(out)
end

local function lanes_view(g)
  local kids = { w = g.w, h = g.h, key = "board-lanes" }
  for _, l in ipairs(g.labels or {}) do
    kids[#kids + 1] = text{ l.name, size = "xs", color = l.color, truncate = true, max_w = g.x0 - 16, pin = { left = 0, top = l.y - 8 } }
  end
  kids.paint = g.paint
  return column(kids)
end

local function fileview(f, sb)
  local kids = { gap = 4, w = "full",
    row{ gap = 8, w = "full", px = 4, py = 4, radius = "xs", key = "board-file-" .. f.path, on = "file", arg = f.path, hover = "hover-subtle", help = f.open and "Hide its diff" or "Show its diff",
      icon{ f.open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
      text{ f.path, size = "xs", mono = true, truncate = true, grow = 1 },
      f.binary and text{ "binary", size = "xs", color = "text-muted" } or false,
      f.added ~= nil and text{ "+" .. tostring(f.added), size = "xs", mono = true, color = "green" } or false,
      f.removed ~= nil and text{ "-" .. tostring(f.removed), size = "xs", mono = true, color = "red" } or false },
  }
  if f.open then
    local d = { gap = 0, w = "full", key = "board-diff-" .. f.path, radius = "xs", fill = "floor", border = "elevated", pad = 4, clip = true }
    for _, g in ipairs(f.groups or {}) do
      local color, fill = "text", nil
      if g.kind == "add" then color, fill = "green", "green-tint"
      elseif g.kind == "del" then color, fill = "red-text", "red-tint"
      elseif g.kind == "hunk" then color = "text-muted" end
      d[#d + 1] = column{ w = "full", px = 4, fill = fill, text{ g.text, mono = true, size = "xs", wrap = true, color = color } }
    end
    if nz(f.note) then d[#d + 1] = note(f.note, "amber") end
    if #(f.groups or {}) == 0 and not nz(f.note) then d[#d + 1] = note("No change to show (a mode change or an empty file).") end
    kids[#kids + 1] = column(d)
  end
  return column(kids)
end

local function detailview(d)
  local kids = { gap = 4, w = "full", px = 16, key = "board-detail" }
  if d.loading then kids[#kids + 1] = note("Reading the commit…")
  elseif nz(d.error) then kids[#kids + 1] = note(d.error, "red")
  else
    if nz(d.body) then kids[#kids + 1] = text{ d.body, size = "sm", wrap = true, color = "text" } end
    for _, f in ipairs(d.files or {}) do kids[#kids + 1] = fileview(f) end
    if (d.files_more or 0) > 0 then kids[#kids + 1] = note(d.files_more .. " more files not listed: truncated", "amber") end
    if d.truncated then kids[#kids + 1] = note("Part of this commit is truncated: git show in a terminal has it whole.") end
  end
  return column(kids)
end

local function commits_view(s, wide)
  local out = { gap = 8, w = "full" }
  out[#out + 1] = section("Commits")
  if s.commits_available == false then
    out[#out + 1] = note(nz(s.commits_why) and s.commits_why or "The commits cannot be read now.", "amber")
    return column(out)
  end
  local g = wide and s.lanes.wide or s.lanes.narrow
  out[#out + 1] = lanes_view(g)
  for _, p in ipairs(s.commits or {}) do
    local tot = p.totals or {}
    out[#out + 1] = row{ gap = 8, w = "full",
      column{ w = 8, h = 8, radius = 4, fill = p.color },
      text{ p.thread, size = "sm", weight = "semibold", color = p.color, truncate = true },
      text{ (tot.commits or 0) .. " commits · " .. (tot.files or 0) .. " files · +" .. (tot.added or 0) .. " -" .. (tot.removed or 0), size = "xs", color = "text-muted", truncate = true, grow = 1 },
      p.merged and badge{ "merged", tone = "green", help = "Merged into the main checkout's branch" } or false }
    if #(p.commits or {}) == 0 then out[#out + 1] = note("No commits yet.", "text-dim") end
    for _, c in ipairs(p.commits or {}) do
      out[#out + 1] = row{ gap = 8, w = "full", px = 4, h = 28, radius = "xs", key = "board-side-commit-" .. c.hash, on = "commit", arg = c.hash, hover = "hover-subtle",
        fill = c.open and "active-bg" or nil, help = c.open and "Fold it" or "Open it: its files and their diff",
        icon{ c.open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
        text{ c.short or "", size = "xs", mono = true, color = "text-muted" },
        text{ c.subject or "", size = "sm", color = "text-bright", truncate = true, grow = 1, help = c.subject },
        text{ (c.files or 0) .. "f", size = "xs", color = "text-muted", help = (c.files or 0) .. " files changed" },
        text{ "+" .. tostring(c.added or 0), size = "xs", mono = true, color = "green" },
        text{ "-" .. tostring(c.removed or 0), size = "xs", mono = true, color = "red" } }
      if c.open and c.detail then out[#out + 1] = detailview(c.detail) end
    end
  end
  return column(out)
end

local TABS = { { "conversation", "Conversation" }, { "notes", "Team and notes" }, { "commits", "Commits" } }

return {
  render = function(_, data)
    local ui = data.board_ui or {}
    local wide = overlay_wide(data)
    local ov = (data.app or {}).overlay or {}
    local ow, oh = ov.w or 1000, ov.h or 600
    CONV_W = wide and (ow - 360 - 16 - 24) or (ow - 24)
    BUBBLE_MAX = 520
    local s = ui.side
    local sb = s and s.sandbox or true
    local picker = picker_button(ui, s, ow, wide)
    -- The picker's list over the body, and under it a layer that closes it
    -- on a press anywhere else (and does nothing else).
    local menu = picker_menu(ui, ow, oh, wide)
    local dismiss = menu and column{ pin = { left = 0, top = 0, right = 0, bottom = 0 }, key = "board-picker-dismiss", on = "picker_close", opacity = 0.01, help = "Close the list of teams" } or false
    local function page(kids)
      kids.h = "full"; kids.gap = 8; kids.w = "full"
      kids[#kids + 1] = dismiss
      kids[#kids + 1] = menu
      return column(kids)
    end
    if not s then
      return page{ picker, filters(ui, wide), conversation(ui, true) }
    end
    local side = column{ gap = 16, w = "full", team_view(s, ui), notes_view(s, sb), commits_view(s, wide) }
    if wide then
      return page{ picker, filters(ui, wide),
        row{ h = "full", gap = 16, w = "full", align = "stretch",
          column{ grow = 1, h = "full", min_w = 0, conversation(ui, sb) },
          column{ w = 360, h = "full", scroll{ key = "board-side", side } } } }
    end
    local tabs = { gap = 4, w = "full" }
    for _, t in ipairs(TABS) do
      local on_it = ui.tab == t[1]
      tabs[#tabs + 1] = button{ t[2], variant = on_it and "neutral" or "ghost", color = on_it and "text-bright" or "text-muted", dense = true, on = "tab", arg = t[1], key = "board-tab-" .. t[1] }
    end
    local body
    if ui.tab == "notes" then body = scroll{ key = "board-side", column{ gap = 16, w = "full", team_view(s, ui), notes_view(s, sb) } }
    elseif ui.tab == "commits" then body = scroll{ key = "board-side", commits_view(s, false) }
    else body = conversation(ui, sb) end
    return page{ picker, row(tabs), ui.tab == "conversation" and filters(ui, wide) or false, body }
  end,
  actions = {
    team = function(_, a, id) return intent("board_team", { id = id or a or "all" }) end,
    picker = function() return intent("board_picker", {}) end,
    picker_close = function() return intent("board_picker", { open = false }) end,
    picker_more = function() return intent("board_picker", { more = true }) end,
    group = function(_, g) return intent("board_filter", { group = g }) end,
    thread = function(_, a, v) return intent("board_filter", { thread = v or a or "" }) end,
    search = function(_, _, t) return intent("board_filter", { text = t or "" }) end,
    more = function() return intent("board_more", {}) end,
    expand = function(_, id) return intent("board_expand", { id = id }) end,
    commit = function(_, h) return intent("board_commit", { hash = h }) end,
    file = function(_, p) return intent("board_file", { path = p }) end,
    tab = function(_, t) return intent("board_tab", { tab = t }) end,
    open = function(_, a) return intent("board_open", { what = a.what, thread = a.thread, relay = a.relay }) end,
  },
}
