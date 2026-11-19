-- The Board (an overlay over the pane area, feat/board/): a team's
-- conversation, as sbxw saw it. The person's acts on the right, the lead and
-- the threads on the left, sbxw's own events in the middle; the board's
-- decisions and warnings stand out as labelled notes; a thread's commits
-- fold into one row. Beside it (760 pt and more): the team, the notes that
-- still stand, and the commits per thread with their diff in place.
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

-- Who wrote it: the thread's colour, the lead marked as lead.
local function author(r)
  return row{ gap = 4,
    column{ w = 8, h = 8, radius = 4, fill = r.who_color or "text-dim" },
    text{ r.who or "", size = "sm", weight = "semibold", color = r.is_user and "text-bright" or (r.who_color or "text-bright") },
    r.is_lead and badge{ "lead", tone = "purple", help = "The session that planned this team" } or false,
    nz(r.to) and text{ "→ " .. r.to, size = "xs", color = "text-muted" } or false,
  }
end

local function team_tag(r)
  if not nz(r.team_label) then return false end
  return row{ gap = 4, max_w = 220,
    column{ w = 8, h = 8, radius = 2, fill = r.team_color or "text-dim" },
    text{ r.team_label, size = "xs", color = "text-muted", truncate = true },
  }
end

local function link(l, key)
  if not l then return false end
  return button{ l.label, variant = "ghost", tone = "blue", dense = true, on = "open", arg = { what = l.what, thread = l.thread, relay = l.relay }, key = key }
end

-- A message's box, on its side.
-- The conversation column's width, set by render from the overlay's.
local CONV_W = 620

local function bubble(r, kids, opts)
  opts = opts or {}
  local right = r.side == "right"
  local box = { gap = 4, px = 12, py = 8, radius = "md", max_w = math.min(620, CONV_W - 24), key = "board-msg-" .. (r.id or ""),
    fill = opts.fill or (right and "surface-alt" or "surface"), border = opts.border or "elevated" }
  local head = row{ gap = 8, w = "full", author(r), team_tag(r), r.guess and text{ "team inferred", size = "xs", color = "text-dim", help = "From before entries named their plan: placed by its time and author" } or false, spacer{}, when(r) }
  if right then
    head = row{ gap = 8, w = "full", when(r), spacer{}, team_tag(r), nz(r.to) and text{ "to " .. r.to, size = "xs", color = "text-muted" } or false, text{ "You", size = "sm", weight = "semibold", color = "text-bright" } }
  end
  box[#box + 1] = head
  for _, k in ipairs(kids) do box[#box + 1] = k end
  return row{ w = "full", justify = right and "end" or "start", column(box) }
end

local function questions(r, sb)
  local out = { gap = 8, w = "full" }
  for _, q in ipairs(r.questions or {}) do
    local kids = { gap = 2, w = "full", px = 8, py = 4, radius = "xs", fill = "floor",
      text{ (q.id or "") .. (nz(q.title) and (" · " .. q.title) or ""), size = "sm", weight = "semibold", color = "text-bright" },
      words(q.text or "", sb, { size = "sm" }),
    }
    if nz(q.recommendation) then kids[#kids + 1] = text{ "Suggested: " .. q.recommendation, size = "sm", color = "text-muted", wrap = true } end
    if nz(q.answer) then
      kids[#kids + 1] = row{ gap = 4, w = "full", icon{ "check", size = 12, color = "green" }, text{ "Answer: " .. q.answer, size = "sm", color = "text-bright", wrap = true, grow = 1 } }
    end
    out[#out + 1] = column(kids)
  end
  return column(out)
end

local NOTE_LOOK = {
  board_decision = { "green-tint", "green" }, board_warning = { "amber-tint", "amber" },
  board_question = { "blue-tint", "blue" }, board_handoff = { "surface-alt", "elevated" }, conflict = { "red-tint", "red-border" },
}

local function rowview(r, sb)
  if r.type == "day" then
    return row{ gap = 8, w = "full", py = 4, key = "board-" .. r.id, column{ grow = 1, h = 1, fill = "elevated" }, text{ r.label, size = "xs", color = "text-muted" }, column{ grow = 1, h = 1, fill = "elevated" } }
  elseif r.type == "system" then
    return row{ gap = 8, w = "full", justify = "center", key = "board-msg-" .. (r.id or ""),
      icon{ ICON[r.tone] or "info", size = 12, color = r.tone == "neutral" and "text-muted" or r.tone },
      text{ r.text or "", size = "sm", color = "text-muted", wrap = true, max_w = math.min(560, CONV_W - 160) },
      team_tag(r), when(r), link(r.link, "board-link-" .. (r.id or "")) }
  elseif r.type == "note" then
    local look = NOTE_LOOK[r.look or r.kind] or { "surface", "elevated" }
    local kids = {
      row{ gap = 4, icon{ ICON[r.tone] or "info", size = 12, color = r.tone == "neutral" and "text-muted" or r.tone }, text{ r.label, size = "sm", weight = "semibold", color = "text-bright" },
        nz(r.note_via) and text{ r.note_via, size = "xs", color = "text-muted", help = "Sent through sbxw with team_note" } or false },
      words(r.text or "", sb),
      fold_button(r),
    }
    local files = r.files or {}
    if #files > 0 then kids[#kids + 1] = text{ table.concat(files, ", "), size = "xs", mono = true, color = "text-muted", wrap = true } end
    return bubble(r, kids, { fill = look[1], border = look[2] })
  elseif r.type == "card" then
    local chips = { gap = 4, wrap = true, w = "full" }
    for _, t in ipairs(r.threads or {}) do
      chips[#chips + 1] = row{ gap = 4, px = 8, h = 24, radius = "pill", fill = "floor", column{ w = 8, h = 8, radius = 4, fill = t.color }, text{ t.name, size = "xs", color = "text-bright" } }
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
        kids[#kids + 1] = column{ gap = 2, w = "full", text{ t.name, size = "sm", weight = "semibold", color = t.color }, words(t.goal or "", sb, { size = "sm" }) }
      end
    end
    return bubble(r, kids, { fill = "surface-alt", border = "purple" })
  elseif r.type == "status" then
    local kids = {}
    if r.state then
      kids[#kids + 1] = row{ gap = 8, w = "full", badge{ r.state.word, tone = r.state.tone }, nz(r.text) and words(r.text, sb, { size = "sm" }) or false }
    else
      kids[#kids + 1] = row{ gap = 8, w = "full", nz(r.label) and r.side ~= "right" and text{ r.label, size = "sm", weight = "semibold", color = r.tone == "neutral" and "text-bright" or r.tone } or false,
        nz(r.text) and (r.side == "right" and text{ r.text, size = "sm", wrap = true, color = "text" } or words(r.text, sb, { size = "sm" })) or false }
    end
    kids[#kids + 1] = fold_button(r)
    return bubble(r, kids)
  elseif r.type == "question" then
    local kids = { text{ r.label or "", size = "sm", weight = "semibold", color = r.tone == "neutral" and "text-bright" or r.tone } }
    if nz(r.text) then kids[#kids + 1] = r.side == "right" and text{ r.text, size = "sm", wrap = true, color = "text" } or words(r.text, sb, { size = "sm" }) end
    kids[#kids + 1] = fold_button(r)
    if r.questions then kids[#kids + 1] = questions(r, sb) end
    if nz(r.hidden_text) then kids[#kids + 1] = note(r.hidden_text, "text-dim") end
    kids[#kids + 1] = link(r.link, "board-link-" .. (r.id or ""))
    return bubble(r, kids)
  elseif r.type == "message" then
    return bubble(r, {
      row{ gap = 4, icon{ "chat", size = 12, color = "text-muted" }, text{ r.label, size = "xs", color = "text-muted", help = "Sent with SendMessage between the team's sessions; sbxw shows what its hook saw" } },
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
  local list = { gap = 8, w = "full", px = 4, folds = true, key = "board-rows" }
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
    local ow = ((data.app or {}).overlay or {}).w or 1000
    CONV_W = wide and (ow - 360 - 16 - 24) or (ow - 24)
    local items = { { id = "all", label = "All teams", icon = "grid" } }
    for _, t in ipairs(ui.teams or {}) do items[#items + 1] = { id = t.id, label = t.label .. " (" .. t.state .. ")" } end
    local s = ui.side
    local sb = s and s.sandbox or true
    local picker = row{ key = "title-slot", gap = 8,
      column{ w = 8, h = 8, radius = 2, fill = (s and s.color) or "text-dim", key = "board-team-color" },
      select{ id = "board-team", value = ui.team or "all", items = items, on = "team", w = wide and 360 or math.max(120, math.min(220, ow - 200)) },
      s and badge{ s.state or "", tone = s.tone or "neutral" } or false }
    local head = { gap = 8, w = "full", picker, filters(ui, wide) }
    if not s then
      head[#head + 1] = conversation(ui, true)
      return column{ h = "full", gap = 8, w = "full", table.unpack(head) }
    end
    local side = column{ gap = 16, w = "full", team_view(s, ui), notes_view(s, sb), commits_view(s, wide) }
    if wide then
      return column{ h = "full", gap = 8, w = "full", picker, filters(ui, wide),
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
    return column{ h = "full", gap = 8, w = "full", picker, row(tabs), ui.tab == "conversation" and filters(ui, wide) or false, body }
  end,
  actions = {
    team = function(_, _, id) return intent("board_team", { id = id or "all" }) end,
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
