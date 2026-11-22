-- The Dashboard (the feature `board`: an overlay over the pane area, feat/board/): a team's
-- conversation, as sbxw saw it. The person's acts on the right, the lead and
-- the threads on the left, sbxw's own events in the middle; the board's
-- decisions and warnings stand out as labelled notes; a thread's commits
-- fold into one row. One column: above it a summary (the team, its threads,
-- the places one acts, counters that are the filters); the Commits filter
-- shows the lane graph and the commits with their diff at the full width.
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
  return text{ r.when or "", size = "xs", color = "text-muted", help = r.exact }
end

-- The fold, decided here at the width drawn now, in constant steps (the
-- model computed, once per text, what this needs: model.rs's `drawing`,
-- whose `head` this mirrors): a text drawn on more than FOLD_DRAWN lines at
-- the bubble's width (fewer characters a line than its `fold.cpl`) shows
-- its first SHOW_DRAWN, cut on a word when a space is near, and "Show
-- more". Decided per frame, so a resize or a press elsewhere never refolds a
-- bubble from a width known earlier. Never a walk over a text here: a
-- string pattern costs the instruction budget a step per character.
local FOLD_DRAWN, SHOW_DRAWN, CHAR_W, HEAD_BACK = 8, 6, 6.0, 48
local function ulen(s) return utf8.len(s) or #s end
local WS = { [9] = true, [10] = true, [11] = true, [12] = true, [13] = true, [32] = true }
local function head(t, f, c)
  local left, stop = SHOW_DRAWN, 0
  for _, l in ipairs(f.lines or {}) do
    local s, nb, nc = l[1], l[2], l[3]
    local n = math.max(1, math.ceil(nc / c))
    if n > left then
      local i = math.min(nb, (utf8.offset(t, left * c + 1, s) or (s + nb)) - s)
      local p, low = i - 1, math.max(math.floor(i / 2) + 1, i - HEAD_BACK)
      while p >= low do
        if string.byte(t, s + p) == 32 then i = p break end
        p -= 1
      end
      stop = s + i - 1
      break
    end
    stop = s + nb - 1
    left -= n
    if left == 0 then break end
  end
  while stop > 0 and WS[string.byte(t, stop)] do stop -= 1 end
  return string.sub(t, 1, stop) .. "…"
end

local function fold_button(r)
  if not r.fold then return false end
  return button{ r.fold.open and "Show less" or "Show more", variant = "ghost", tone = "blue", dense = true, on = "expand", arg = r.fold.key, key = "board-fold-" .. r.fold.key }
end

-- The conversation column's width, set by render from the overlay's, and a
-- bubble's widest. The rule (the same as model::bubble_max, which the fold
-- reads; a test holds the two together): in one column (under 760 pt) 520 pt;
-- with room, 62 percent of the column, at least 520 pt and at most BUBBLE_CAP,
-- never past the column less its gutters. Notes, cards and tinted reports use
-- it too, so the column reads as one system.
local CONV_W = 620
local BUBBLE_MAX = 520
local BUBBLE_CAP = 720
local function bubble_w() return math.max(120, math.min(BUBBLE_MAX, CONV_W - 24)) end

-- A team in "All teams": its colour and its threads' names (its summary in
-- the tooltip), never a cut sentence.
local function team_tag(r)
  if not nz(r.team_label) then return false end
  return row{ gap = 4, max_w = math.max(80, math.min(240, CONV_W - 220)), help = r.team_help,
    column{ w = 8, h = 8, radius = "mark", fill = r.team_color or "text-muted" },
    text{ r.team_label, size = "xs", mono = true, color = "text-muted", truncate = true },
  }
end

-- The author line, above the bubble: dot, name (mono, the thread's colour),
-- the role only when the name does not say it, "to" whom, the time. The
-- person's: on the right, "You" last.
local function author_line(r)
  local nw = math.max(60, math.min(260, CONV_W - 200))
  if r.side == "right" then
    return row{ gap = 8, w = "full", justify = "end", key = "board-author-" .. (r.id or ""),
      team_tag(r), when(r),
      nz(r.to) and text{ "to " .. r.to, size = "xs", color = "text-muted", truncate = true, max_w = nw } or false,
      text{ "You", size = "sm", weight = "semibold", color = "text-bright" } }
  end
  return row{ gap = 8, w = "full", key = "board-author-" .. (r.id or ""),
    column{ w = 8, h = 8, radius = "pill", fill = r.who_color or "text-muted" },
    text{ r.who or "", size = "sm", mono = true, weight = "semibold", color = r.who_color or "text-bright", truncate = true, max_w = nw },
    nz(r.role) and badge{ r.role, tone = "purple", help = "The session that planned this team" } or false,
    nz(r.to) and text{ "to " .. r.to, size = "xs", color = "text-muted", truncate = true, max_w = nw } or false,
    r.guess and text{ "team inferred", size = "xs", color = "text-muted", help = "From before entries named their plan: placed by its time and author" } or false,
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
    row{ w = "full", justify = right and "end" or "start", px = right and 0 or 16, column(box) } }
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

-- A row as drawn now: a long text folded at this width (a view over the
-- row: the data is not touched). CPL, characters a line of a bubble holds
-- now, is set by render.
local CPL = 82
local function at_width(r)
  local f = r.fold
  if not f or not nz(r.text) then return r end
  if CPL >= (f.cpl or 0) then return setmetatable({ fold = false }, { __index = r }) end
  if f.open then return r end
  return setmetatable({ text = head(r.text, f, CPL) }, { __index = r })
end

local function rowview(r, sb)
  r = at_width(r)
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
    return row{ gap = 8, w = "full", px = 4, key = "board-msg-" .. (r.id or ""),
      help = n > 1 and (n .. " changes of state: " .. (r.trail or "") .. " (from " .. (r.first_when or "") .. ")") or nil,
      column{ w = 8, h = 8, radius = "pill", fill = r.who_color or "text-muted" },
      text{ r.who or "", size = "sm", mono = true, color = r.who_color or "text-muted", truncate = true, max_w = math.max(60, math.min(260, CONV_W - 220)) },
      icon{ "chevron-right", size = 12, color = "text-muted" },
      badge{ r.state.word, tone = r.state.tone },
      n > 1 and text{ "×" .. n, size = "xs", color = "text-muted" } or false,
      team_tag(r), when(r) }
  elseif r.type == "note" then
    local look = NOTE_LOOK[r.look or r.kind] or { "surface", "elevated" }
    local kids = {
      row{ gap = 8, wrap = true,
        kind_line(r.label, { icon = ICON[r.tone] or "info", icon_color = r.tone == "neutral" and "text-muted" or r.tone, color = "text-bright", weight = "semibold",
          extra = r.note_via, extra_help = "Sent through sbxw with team_note" }),
        -- Still standing: the summary counts it, the filter finds it, this marks it.
        r.standing and badge{ "standing", tone = r.tone == "amber" and "amber" or "green", key = "board-standing-" .. (r.id or ""), help = "Still standing: the summary counts it" } or false },
      words(r.text or "", sb),
      fold_button(r),
    }
    local files = r.files or {}
    if #files > 0 then kids[#kids + 1] = text{ table.concat(files, ", "), size = "xs", mono = true, color = "text-muted", wrap = true } end
    return bubble(r, kids, { fill = look[1], border = look[2] })
  elseif r.type == "card" then
    local chips = { gap = 4, wrap = true }
    for _, t in ipairs(r.threads or {}) do
      chips[#chips + 1] = row{ gap = 4, px = 8, h = 24, radius = "pill", fill = "floor", column{ w = 8, h = 8, radius = "pill", fill = t.color }, text{ t.name, size = "xs", mono = true, color = "text-bright" } }
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
      -- The kind label at the left, the state chip at the right, a gap of at least 12 between.
      kids[#kids + 1] = row{ gap = 4, w = "full", align = "center", key = "board-head-" .. (r.id or ""),
        text{ "status report", size = "xs", color = "text-muted", key = "board-kind-" .. (r.id or "") },
        column{ grow = 1, min_w = 12, h = 1 },
        badge{ r.state.word, tone = r.state.tone, key = "board-state-" .. (r.id or "") } }
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
    if nz(r.hidden_text) then kids[#kids + 1] = note(r.hidden_text, "text-muted") end
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
      column{ w = 8, h = 8, radius = "pill", fill = r.who_color },
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

-- The room it leaves keeps the title ("Dashboard", 14/600, about 76 pt)
-- and the overlay's cross on the row at every width.
local function picker_w(ow, wide)
  if wide then return math.min(420, ow - 296) end
  return math.max(104, math.min(300, ow - 256))
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
  -- The scope's capsule (scope.rs's look, the team sandbox's group colour),
  -- as every scoped screen draws it; neutral for All teams.
  local lk = (s and s.look) or {}
  return row{ key = "title-slot", gap = 8,
    row{ key = "board-row-picker", on = "picker", w = bw, h = 28, px = 12, gap = 8, radius = "pill", border = lk.border or "control-border", fill = lk.fill or "surface",
      -- No tooltip while its list is open: it would cover the first rows.
      help = (not p.open) and ((nz(summary) and summary or "Pick a team") .. " (pick another team)") or nil,
      (s or not ui.all) and column{ w = 8, h = 8, radius = "mark", fill = color or "text-muted", key = "board-team-color" } or icon{ "grid", size = 14, color = lk.muted or "text-muted", key = "board-team-color" },
      text{ names or "", size = "sm", weight = "medium", mono = true, color = lk.text or "text-bright", truncate = true, max_w = math.floor(bw * 0.55) },
      nz(summary) and wide and text{ summary, size = "xs", color = lk.muted or "text-muted", truncate = true, grow = 1 } or spacer{},
      icon{ "chevron-down", size = 12, color = lk.muted or "text-muted" } },
    s and badge{ s.state or "", tone = s.tone or "neutral" } or false }
end

local function picker_row(t, i, p, mw)
  local hl = (p.hl or 0) == i
  local current = p.current == t.id
  -- A tooltip only when the summary is cut here (none over a row that shows it all).
  local shown = nz(t.summary) and two_lines(t.summary, mw) or nil
  return column{ key = "board-row-team-" .. t.id, on = "team", arg = t.id, gap = 2, px = 8, py = 8, radius = "xs", w = "full",
    fill = (hl or current) and "active-bg" or nil, hover = "hover-subtle", selected = current, help = (shown and shown ~= t.summary) and t.summary or nil,
    row{ gap = 8, w = "full",
      column{ w = 8, h = 8, radius = "mark", fill = t.color or "text-muted" },
      text{ t.names or "", size = "sm", mono = true, weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
      current and icon{ "check", size = 12, color = "accent" } or false,
      nz(t.state) and badge{ t.state, tone = t.tone or "neutral" } or false,
      nz(t.when) and text{ t.when, size = "xs", color = "text-muted" } or false },
    shown and text{ shown, size = "xs", color = "text-muted", wrap = true } or false }
end

local function picker_menu(ui, ow, oh, wide)
  local p = ui.picker or {}
  if not p.open then return false end
  local mw = menu_w(ow, wide)
  local list = { gap = 2, w = "full", key = "board-picker-list", group = "list" }
  list[#list + 1] = picker_row({ id = "all", names = "All teams", summary = "Every team's latest messages, each with its team", color = "text-muted" }, 0, p, mw)
  -- Teams that need you first (the core's order), then the finished ones
  -- under a quiet heading.
  local headed = false
  for i, t in ipairs(ui.teams or {}) do
    if t.live == false and not headed and i > 1 then
      list[#list + 1] = row{ w = "full", gap = 8, px = 8, key = "board-picker-finished",
        text{ "Finished", size = "xs", weight = "semibold", color = "text-muted" }, column{ grow = 1, h = 1, fill = "elevated" } }
      headed = true
    end
    list[#list + 1] = picker_row(t, i, p, mw)
  end
  if (p.more or 0) > 0 then
    list[#list + 1] = row{ w = "full", justify = "center", button{ "Show " .. p.more .. " more", variant = "ghost", tone = "blue", dense = true, on = "picker_more", key = "board-picker-more" } }
  end
  local est = 8 + 58 * (#(ui.teams or {}) + 1) + ((p.more or 0) > 0 and 32 or 0) + 24
  local h = math.max(80, math.min(est, (oh or 600) - 24))
  return column{ pin = { left = 0, top = 0 }, w = mw, key = "board-picker", fill = "surface", border = "elevated", radius = "md", pad = 4,
    scroll{ h = h, key = "board-picker-scroll", column(list) } }
end

local GROUPS = { { "all", "All" }, { "notes", "Decisions and warnings" }, { "questions", "Questions" }, { "messages", "Messages" }, { "commits", "Commits" }, { "status", "Status" } }
local GROUP_HELP = { messages = "What the lead and threads sent each other with SendMessage, as sbxw saw it: from the day the daemon had this version, none from before" }

local function filters(ui, wide)
  local seg = { gap = 4, w = "full", wrap = true }
  -- The Dashboard's two views; the Kanban is its own screen (kanban.luau).
  seg[#seg + 1] = select{ id = "board-view", value = "conversation", items = { { id = "conversation", label = "Conversation" }, { id = "kanban", label = "Kanban" } }, on = "view", switch = true,
    help = "The Dashboard's two views: the team's conversation, or the Kanban of its tickets" }
  for _, g in ipairs(GROUPS) do
    local on_it = ui.group == g[1]
    seg[#seg + 1] = button{ g[2], variant = on_it and "neutral" or "ghost", color = on_it and "text-bright" or "text-muted", dense = true, on = "group", arg = g[1], key = "board-group-" .. g[1], help = GROUP_HELP[g[1]] }
  end
  seg[#seg + 1] = input{ id = "board-search", value = ui.text or "", placeholder = "Search the conversation", on = "search", grow = 1, min_w = 160 }
  return row(seg)
end

local function conversation(ui, sb)
  local list = { gap = 4, w = "full", px = 4, folds = true, key = "board-rows" }
  if ui.capped then
    -- As many rows as one view draws: older ones through a filter or a search.
    list[#list + 1] = row{ w = "full", justify = "center", key = "board-capped", note("The newest " .. #(ui.rows or {}) .. " rows are drawn. A filter, a thread or a search finds earlier ones.", "text-muted") }
  elseif ui.older then
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
  -- Follows its end (new events), until a fold is pressed: then growing or
  -- shrinking a bubble moves only what is under it (see `held`, mod.rs).
  return scroll{ key = "board-convo", stick_bottom = not ui.held, column(list) }
end

-- ── The summary (one column: replaces the side column) ──────────────────
-- A section between two hairlines (not a card inside the tile).
-- Under the title (which has the team's state), above the filters: its lead, each
-- thread as a chip (press: only that thread; its terminal and its session
-- beside it, shown on hover and focus), the places one acts, and counters
-- that are the filters below (a press applies one; nothing is listed
-- again). "Details" opens, in place, what the conversation does not show:
-- each thread's worktree and model. Narrow (under 760 pt): one wrapping
-- line, chips and counters lose their words (their tooltips keep them),
-- the links become icons, a thread's terminal and session move to Details.

-- A thread's two links: 20 pt icons (on a chip, shown on hover and focus,
-- their room kept so nothing reflows).
local function icon_link(ic, what, t, key, help, reveal)
  return row{ key = key, on = "open", arg = { what = what, thread = t.name }, w = 20, h = 20, radius = "xs", justify = "center",
    hover = "hover-subtle", reveal = reveal, help = help, icon{ ic, size = 12, color = "text-muted" } }
end
local function term_link(t, reveal)
  return icon_link("terminal", "terminal", t, "board-term-" .. t.name, t.name .. ": its terminal", reveal)
end
local function mon_link(t, ui, reveal)
  return ui.monitor and icon_link("inbox", "monitor", t, "board-mon-" .. t.name, t.name .. ": its session in Sessions", reveal) or false
end

-- About how wide a chip draws (mono 11 pt name, 11 pt state, the two icons).
local function chip_w(t, ui, narrow)
  local w = 8 + 8 + 6 + math.min(narrow and 120 or 180, ulen(t.name) * 6.8) + 6 + 4
  if not narrow then w = w + ulen(t.state or "") * 6.2 + 6 + 20 + (ui.monitor and 20 or 0) end
  return w
end

local function thread_chip(t, ui, narrow)
  local tone_c = t.tone == "neutral" and "text-muted" or t.tone
  return row{ key = "board-chip-" .. t.name, gap = 0, radius = "pill", h = 26, px = 0, hover_group = true, pad = 0,
    fill = t.selected and "active-bg" or "floor", border = t.selected and "accent" or "elevated",
    row{ key = "board-thread-" .. t.name, on = "thread", arg = t.selected and "" or t.name, selected = t.selected,
      gap = 4, px = 8, h = 24, radius = "pill", hover = "hover-subtle",
      help = t.selected and "Every thread again" or ("Only " .. t.name .. " in the conversation (" .. t.state .. ")"),
      dot{ tone = t.tone },
      text{ t.name, size = "xs", mono = true, weight = "semibold", color = t.color, truncate = true, max_w = narrow and 120 or 180 },
      (not narrow) and text{ t.state, size = "xs", color = tone_c } or false },
    -- Narrow: these two are in Details (a chip stays small).
    (not narrow) and term_link(t, true) or false,
    (not narrow) and mon_link(t, ui, true) or false,
  }
end

-- A counter: how many, and the filter of the row below it applies.
local function counter(ui, key, group, icon_name, tone, n, word, help, extra)
  local on_it = ui.group == group
  return row{ key = "board-count-" .. key, on = "group", arg = on_it and "all" or group, selected = on_it,
    gap = 4, px = 8, h = 24, radius = "pill", hover = "hover-subtle",
    fill = on_it and "active-bg" or nil, border = on_it and "accent" or nil,
    help = on_it and ("Shown: " .. help .. ". Press for every kind again") or ("Show " .. help),
    icon{ icon_name, size = 12, color = tone },
    text{ tostring(n), size = "sm", weight = "semibold", color = "text-bright" },
    word and text{ word, size = "xs", color = "text-muted" } or false,
    extra or false }
end

local function summary(s, ui, narrow)
  local c = s.counts or {}
  local head = { gap = 8, w = "full", wrap = true, key = "board-summary-team" }
  -- (The team's state is beside the picker, in the title: not twice.)
  head[#head + 1] = row{ key = "board-chip-lead", gap = 0, radius = "pill", h = 26, px = 0,
    fill = ui.thread == "lead" and "active-bg" or "floor", border = ui.thread == "lead" and "accent" or "elevated",
    row{ key = "board-thread-lead", on = "thread", arg = ui.thread == "lead" and "" or "lead", selected = ui.thread == "lead",
      gap = 4, px = 8, h = 24, radius = "pill", hover = "hover-subtle",
      help = ui.thread == "lead" and "Every thread again" or ("Only the lead in the conversation" .. (nz(s.lead) and (": " .. s.lead) or "")),
      badge{ "lead", tone = "purple" },
      text{ s.lead or "the sandbox's main session", size = "xs", color = "text-bright", truncate = true, max_w = narrow and 100 or 200 } } }
  -- As many chips as the line holds (two lines when narrow); the rest
  -- behind "+n", which opens Details (where every thread is listed and
  -- pressable). A selected thread is always shown.
  local room = (CONV_W - 24) * (narrow and 2 or 1) - 8 - math.min(narrow and 100 or 200, ulen(s.lead or "the sandbox's main session") * 6.8) - 60 - 70
  local used, hidden = 0, 0
  local threads = s.threads or {}
  for i, t in ipairs(threads) do
    local w = chip_w(t, ui, narrow) + 8
    if t.selected or used + w <= room then
      head[#head + 1] = thread_chip(t, ui, narrow)
      used = used + w
    else
      hidden = hidden + 1
    end
  end
  if hidden > 0 then
    head[#head + 1] = row{ key = "board-chip-more", on = "expand", arg = "summary", h = 24, px = 8, radius = "pill", fill = "floor", border = "elevated", hover = "hover-subtle",
      help = hidden .. " more thread" .. (hidden == 1 and "" or "s") .. ": Details lists them all", text{ "+" .. hidden, size = "xs", weight = "semibold", color = "text-muted" } }
  end
  local links = { gap = 2, key = "board-links" }
  local function lnk(label, ic, what, key, help)
    if narrow then return button{ icon = ic, variant = "ghost", dense = true, on = "open", arg = { what = what }, key = key, help = label .. (help and (": " .. help) or "") } end
    return button{ label, variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = what }, key = key, help = help }
  end
  links[#links + 1] = lnk("Plan dialog", "leader", "plan", "board-plan", "Approve, follow or merge the plan there")
  links[#links + 1] = lnk("Threads", "threads", "threads", "board-threads", "The Threads panel of " .. (s.sandbox or ""))
  links[#links + 1] = lnk("Worktrees", "branch", "worktrees", "board-worktrees", "The worktrees of " .. (s.sandbox or ""))

  local counts = { gap = 4, w = "full", wrap = true, key = "board-summary-counts" }
  local function w(word) return (not narrow) and word or nil end
  if (c.decisions or 0) > 0 then counts[#counts + 1] = counter(ui, "decisions", "notes", "check", "green", c.decisions, w(c.decisions == 1 and "decision" or "decisions"), c.decisions .. " standing decision" .. (c.decisions == 1 and "" or "s") .. " (Decisions and warnings)") end
  if (c.warnings or 0) > 0 then counts[#counts + 1] = counter(ui, "warnings", "notes", "warning", "amber", c.warnings, w(c.warnings == 1 and "warning" or "warnings"), c.warnings .. " standing warning" .. (c.warnings == 1 and "" or "s") .. " (Decisions and warnings)") end
  if (c.waiting or 0) > 0 then counts[#counts + 1] = counter(ui, "questions", "questions", "ask", "amber", c.waiting, w("waiting"), c.waiting .. " question" .. (c.waiting == 1 and "" or "s") .. " waiting for an answer in Requests (Questions)") end
  if (c.commits or 0) > 0 then
    counts[#counts + 1] = counter(ui, "commits", "commits", "history", "text-muted", c.commits, w(c.commits == 1 and "commit" or "commits"),
      c.commits .. " commits, +" .. (c.added or 0) .. " -" .. (c.removed or 0) .. " lines (Commits)",
      (not narrow) and row{ gap = 4, text{ "+" .. tostring(c.added or 0), size = "xs", mono = true, color = "green" }, text{ "-" .. tostring(c.removed or 0), size = "xs", mono = true, color = "red" } } or nil)
  end
  if #counts == 0 then counts[#counts + 1] = text{ "No standing decision or warning, no question waiting, no commit yet.", size = "xs", color = "text-muted" } end
  counts[#counts + 1] = spacer{}
  counts[#counts + 1] = row(links)
  local open = ui.details
  counts[#counts + 1] = button{ (not narrow) and (open and "Less" or "Details") or nil, icon = open and "chevron-down" or "chevron-right", variant = "ghost", dense = true, on = "expand", arg = "summary",
    key = "board-summary-details", help = open and "Hide the threads' worktrees and models" or "The threads' worktrees and models" }

  -- A section of the tile, not a card in it: a hairline above and below,
  -- its rows at the tile's own left edge (as the picker and the filters).
  local kids = { gap = 8, w = "full", key = "board-summary", folds = true, divider{} }
  if narrow then
    -- One wrapping line: chips, counts, link icons, Details.
    for _, k in ipairs(counts) do head[#head + 1] = k end
    kids[#kids + 1] = row(head)
  else
    kids[#kids + 1] = row(head)
    kids[#kids + 1] = row(counts)
  end
  if open then
    local d = { gap = 2, w = "full", key = "board-summary-more" }
    for _, t in ipairs(s.threads or {}) do
      d[#d + 1] = row{ gap = 8, w = "full", key = "board-detail-" .. t.name,
        row{ key = "board-detail-thread-" .. t.name, on = "thread", arg = t.selected and "" or t.name, selected = t.selected, gap = 4, px = 4, h = 22, radius = "xs", hover = "hover-subtle",
          help = t.selected and "Every thread again" or ("Only " .. t.name .. " in the conversation (" .. (t.state or "") .. ")"),
          dot{ tone = t.tone },
          text{ t.name, size = "xs", mono = true, weight = "semibold", color = t.color, truncate = true, max_w = narrow and 110 or 180 } },
        text{ nz(t.meta) and t.meta or "no model set", size = "xs", mono = true, color = "text-muted", truncate = true, grow = 1, min_w = 0, help = t.meta },
        narrow and term_link(t, false) or false,
        narrow and mon_link(t, ui, false) or false }
    end
    if #(s.threads or {}) == 0 then d[#d + 1] = note("No thread yet.", "text-muted") end
    kids[#kids + 1] = column(d)
  end
  kids[#kids + 1] = divider{}
  return column(kids)
end

local function section(title, extra)
  return row{ gap = 8, w = "full", text{ title, size = "sm", weight = "semibold", color = "text-muted" }, extra or false }
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
  kids[#kids + 1] = divider{}
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

-- The Commits filter, a team shown: the lane graph on top, each thread's
-- totals, its commits with their diff in place, the conversation's whole
-- width (it was the side column's). In "All" the commits stay folded rows.
local function commits_view(s)
  local out = { gap = 8, w = "full", px = 4, key = "board-commits", folds = true }
  if s.commits_available == false then
    out[#out + 1] = note(nz(s.commits_why) and s.commits_why or "The commits cannot be read now.", "amber")
    return scroll{ key = "board-commits-scroll", column(out) }
  end
  out[#out + 1] = lanes_view(s.lanes)
  for _, p in ipairs(s.commits or {}) do
    local tot = p.totals or {}
    out[#out + 1] = row{ gap = 8, w = "full", py = 4, key = "board-commits-" .. p.thread,
      column{ w = 8, h = 8, radius = "pill", fill = p.color },
      text{ p.thread, size = "sm", weight = "semibold", color = p.color, truncate = true, max_w = 240 },
      text{ (tot.commits or 0) .. " commits · " .. (tot.files or 0) .. " files · +" .. (tot.added or 0) .. " -" .. (tot.removed or 0), size = "xs", color = "text-muted", truncate = true, grow = 1 },
      p.merged and badge{ "merged", tone = "green", help = "Merged into the main checkout's branch" } or false }
    if #(p.commits or {}) == 0 then out[#out + 1] = note("No commits yet.", "text-muted") end
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
  return scroll{ key = "board-commits-scroll", column(out) }
end

return {
  render = function(_, data)
    local ui = data.board_ui or {}
    local wide = overlay_wide(data)
    local ov = (data.app or {}).overlay or {}
    local ow, oh = ov.w or 1000, ov.h or 600
    local s = ui.side
    -- One column: the conversation takes the overlay's width (model::conv_w).
    CONV_W = ow - 24
    BUBBLE_MAX = wide and math.max(520, math.min(BUBBLE_CAP, math.floor(CONV_W * 0.62))) or 520
    CPL = math.max(16, math.floor((bubble_w() - 24) / CHAR_W))
    local sb = s and s.sandbox or true
    local picker = picker_button(ui, s, ow, wide)
    -- The picker's list over the body, and under it a layer that closes it
    -- on a press anywhere else (and does nothing else).
    local menu = picker_menu(ui, ow, oh, wide)
    local dismiss = menu and column{ pin = { left = 0, top = 0, right = 0, bottom = 0 }, key = "board-picker-dismiss", on = "picker_close", opacity = 0.01, help = "Close the list of teams" } or false
    local body
    if s and ui.group == "commits" then body = commits_view(s) else body = conversation(ui, sb) end
    return column{ h = "full", gap = 8, w = "full",
      picker,
      s and summary(s, ui, not wide) or false,
      filters(ui, wide),
      body,
      dismiss, menu }
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
    open = function(_, a) return intent("board_open", { what = a.what, thread = a.thread, relay = a.relay }) end,
    view = function(_, _, v) if v == "kanban" then return intent("kanban_show", {}) end end,
  },
}
