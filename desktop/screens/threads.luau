-- The Threads panel (docked on the right): one sandbox's Claude sessions
-- (the main one, sessions in the main checkout, threads on worktrees) with
-- where each stands, Team mode and its plan, the worktrees, and a shared
-- plan's board. The web UI's Sessions menu and sidebar fold (panes.js),
-- worktrees.js and board.js.
--
-- Everything here asks: closing a thread, approving or merging a plan open
-- the core's own dialog, built from the daemon's data.

local tabs = { { "sessions", "Sessions" }, { "worktrees", "Worktrees" }, { "board", "Board" } }

local function section(title, count)
  return row{ gap = 8, w = "full", pad = 2,
    text{ title, size = "caption", weight = "semibold", color = "text-muted" },
    count and badge{ tostring(count), tone = "neutral" } or false,
  }
end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function plural(n, one, many) return tostring(n) .. " " .. (n == 1 and one or (many or one .. "s")) end

-- The sandbox shown: the one the panel was opened on, else the selected one,
-- else the first running one.
local function shown(data)
  local ui = data.threads_ui or {}
  local names = {}
  for _, s in ipairs(data.sandboxes or {}) do names[s.name] = s end
  local sb = ui.sandbox
  if not sb or not names[sb] then sb = data.app.selected end
  if not sb or not names[sb] then
    sb = nil
    for _, s in ipairs(data.sandboxes or {}) do if s.status == "running" then sb = s.name; break end end
  end
  return sb, sb and names[sb] or nil
end

local function pane_of(data, sb, thread)
  for _, p in ipairs(data.app.panes or {}) do
    if p.sandbox == sb and p.thread == thread and p.mode == "agent" then return p.index end
  end
  return nil
end

local function plan_by_id(data, id)
  for _, p in ipairs(data.plans or {}) do if p.id == id then return p end end
  return nil
end

local function session_row(data, sb, s, running_sb)
  local t = s.thread
  local args = { sandbox = sb, thread = t }
  local key = "ses-" .. (t or "main")
  local pane = pane_of(data, sb, t)
  local glyph = (t == nil or s.kind == "same") and "✦" or "⑂"
  local head = row{ gap = 8, w = "full",
    dot{ tone = s.tone == "muted" and "neutral" or s.tone },
    text{ glyph .. " " .. s.label, weight = "medium", color = "text-bright", truncate = true, grow = 1 },
    s.word ~= "" and badge{ s.word, tone = s.tone == "muted" and "neutral" or s.tone } or false,
    pane and badge{ "P" .. pane, tone = "blue", help = "In pane " .. pane } or false,
  }
  local meta = {}
  if s.kind == "team" then table.insert(meta, "shared worktree") elseif t and s.kind == "same" then table.insert(meta, "main checkout") elseif t then table.insert(meta, ".sbxw-worktrees/" .. t) end
  if s.repo and s.repo ~= "" then table.insert(meta, "in " .. s.repo) end
  if s.model then table.insert(meta, s.model) end
  local buttons = { gap = 4, w = "full" }
  if running_sb then
    table.insert(buttons, button{ pane and "Go to" or (s.running and "Show" or "Start"), icon = "terminal", on = "open", arg = args, key = key .. "-open",
      help = pane and ("In pane " .. pane) or (s.running and "Show it in a pane" or "Start it again in a pane") })
    table.insert(buttons, button{ "Bash", on = "bash", arg = args, key = key .. "-bash", help = t and "A shell in its worktree" or "A shell in the sandbox" })
    if s.running then
      table.insert(buttons, button{ icon = "stop", on = "interrupt", arg = args, key = key .. "-esc", help = "Interrupt: sends Escape, Claude Code stops what it is doing" })
      table.insert(buttons, button{ icon = "link", on = "remote", arg = args, key = key .. "-remote", help = "Remote control: drive this session from claude.ai/code" })
    end
    if t then
      table.insert(buttons, spacer{})
      table.insert(buttons, button{ s.kind == "same" and "Close session" or "Close thread", icon = "close", variant = "danger", on = "close", arg = args, key = key .. "-close" })
    end
  end
  return column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = s.state == "wants" and "amber" or "elevated", key = key,
    head,
    #meta > 0 and text{ table.concat(meta, " · "), size = "sm", color = "text-muted", truncate = true } or false,
    (s.summary and s.summary ~= "") and text{ s.summary, size = "sm", color = "text-soft", wrap = true, pills = sb } or false,
    row(buttons),
  }
end

local function plan_block(data, v, sb)
  local out = { gap = 8, w = "full" }
  if v.pending then
    table.insert(out, row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "amber-tint", border = "amber",
      icon{ "flag", color = "amber" }, text{ "The lead's plan waits for you", color = "text-bright", grow = 1, wrap = true },
      button{ "Review plan", variant = "primary", on = "review", arg = v.pending, key = "plan-review" } })
  end
  if v.merge then
    local p = plan_by_id(data, v.merge) or {}
    local st = (p.merge or {}).state
    table.insert(out, row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "green-tint", border = "green",
      icon{ "check", color = "green" },
      text{ st == "merging" and "Merging the threads…" or (st == "failed" and "The merge stopped" or "The lead proposes merging its threads"), color = "text-bright", grow = 1, wrap = true },
      button{ st == "failed" and "Retry merge" or "Merge…", on = "review", arg = v.merge, key = "plan-merge" } })
  end
  -- Approved plans still at work: their status.
  for _, p in ipairs(data.plans or {}) do
    if p.sandbox == sb and p.state == "approved" and not p.closed and p.id ~= v.merge and not (p.merge and p.merge.state == "merged") then
      local n, done = 0, 0
      for _, t in ipairs(p.threads or {}) do n = n + 1; if t.state == "done" then done = done + 1 end end
      table.insert(out, row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = "elevated", key = "plan-" .. p.id,
        text{ "Plan: " .. done .. " of " .. plural(n, "thread") .. " done", grow = 1, truncate = true },
        button{ "Status", on = "review", arg = p.id, key = "plan-status-" .. p.id } })
    end
  end
  return column(out)
end

local function sessions_tab(data, sb, sbx)
  local v = (data.threads_view or {})[sb] or {}
  local running = sbx and sbx.status == "running"
  local out = { gap = 8, w = "full" }
  -- A failed read: `data.app.errors.threads` (a string, or by sandbox).
  local err = (data.app.errors or {}).threads
  if type(err) == "table" then err = err[sb] end
  if err then table.insert(out, note("Could not read the threads of " .. sb .. ": " .. err, "red"))
  elseif running and v.max == nil then table.insert(out, note("The threads of " .. sb .. " are not read yet.", "amber")) end
  if running then
    table.insert(out, row{ gap = 8, w = "full",
      v.leading and badge{ "★ Team mode on", tone = "purple" } or text{ "Team mode off", size = "sm", color = "text-muted" },
      spacer{},
      v.leading and button{ "End Team mode", on = "lead_end", arg = sb, key = "lead-end" }
        or button{ "Team mode", icon = "sparkle", on = "lead_start", arg = sb, key = "lead-start",
          help = "Make the main session plan the work into threads, have you approve the plan, and follow them" },
    })
  end
  table.insert(out, plan_block(data, v, sb))
  local ses = v.sessions or {}
  local count = (v.max ~= nil) and (tostring(v.open or 0) .. " of " .. tostring(v.max) .. " Claude sessions open") or nil
  table.insert(out, section("Sessions", #ses))
  if count then table.insert(out, note(count .. ((v.open or 0) >= (v.max or 99) and ". No place is free: close one before starting another." or "."),
    (v.open or 0) >= (v.max or 99) and "amber" or nil)) end
  if not running then table.insert(out, note(sb .. " is not running: start it to open its sessions.")) end
  local same, own = {}, {}
  for _, s in ipairs(ses) do
    if s.thread == nil or s.kind == "same" then table.insert(same, s) else table.insert(own, s) end
  end
  if #own > 0 then table.insert(out, text{ "✦ Shared checkout", size = "sm", color = "text-muted", help = "Same files as the main session: no worktree, no branch" }) end
  for _, s in ipairs(same) do table.insert(out, session_row(data, sb, s, running)) end
  if #own > 0 then table.insert(out, text{ "⑂ Own worktree", size = "sm", color = "text-muted", help = "Each thread on a git worktree and branch of its own" }) end
  for _, s in ipairs(own) do table.insert(out, session_row(data, sb, s, running)) end
  if running then
    table.insert(out, row{ gap = 8, w = "full",
      button{ "New thread", icon = "plus", on = "new", arg = { sandbox = sb, kind = "thread" }, key = "thread-new", help = "Another Claude session in its own git worktree and branch" },
      button{ "New session", icon = "plus", on = "new", arg = { sandbox = sb, kind = "session" }, key = "session-new", help = "Another Claude session on the main checkout, beside the main one" },
    })
  end
  return column(out)
end

local function short(sha) return string.sub(sha or "", 1, 7) end

local function worktrees_tab(data, sb)
  local ui = data.threads_ui or {}
  local out = { gap = 8, w = "full" }
  table.insert(out, row{ gap = 8, w = "full",
    text{ "Each thread's branch against the branch its repo's main checkout is on, where a merge lands.", size = "sm", color = "text-muted", wrap = true, grow = 1 },
    button{ icon = "reload", on = "wt_load", arg = sb, key = "wt-reload", help = "Read again" } })
  local w = data.worktrees
  local err = (data.app.errors or {}).worktrees
  if ui.worktrees_for ~= sb or (w == nil and not err) then
    table.insert(out, note("Reading the branches…"))
    return column(out)
  end
  if err or (w and w.ok == false) then
    table.insert(out, note(err or w.error or "Could not read the worktrees", "red"))
    return column(out)
  end
  if #(w.repos or {}) == 0 then
    table.insert(out, note("No thread has a worktree yet. Open one with New thread, or let the lead plan them."))
  end
  for i, r in ipairs(w.repos or {}) do
    local name = (r.path ~= "" and r.path) or ((w.workspace or ""):match("([^/]+)/?$") or "workspace")
    local items = { gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = "elevated", key = "wt-repo-" .. i,
      row{ gap = 8, w = "full", icon{ "folder", size = 12, color = "text-muted" }, text{ name, weight = "medium", color = "text-bright", truncate = true, grow = 1 },
        badge{ r.base or "HEAD", tone = "blue", help = "The branch the main checkout is on: the threads merge into it" } },
    }
    if r.error then table.insert(items, note(r.error, "red")) end
    local dirty = r.dirty or 0
    table.insert(items, text{ "main checkout at " .. short(r.head) .. (dirty > 0 and (" · " .. dirty .. " uncommitted (a merge refuses it)") or " · clean"), size = "sm", color = dirty > 0 and "amber" or "text-muted", wrap = true })
    if #(r.threads or {}) == 0 then table.insert(items, note("No thread branch in this repo.")) end
    for _, t in ipairs(r.threads or {}) do
      local bits = { (t.ahead or 0) .. " ahead", (t.behind or 0) .. " behind" }
      if (t.dirty or 0) > 0 then table.insert(bits, t.dirty .. " uncommitted") end
      local commits = {}
      for k, c in ipairs(t.commits or {}) do
        if k > 3 then break end
        table.insert(commits, text{ short(c.sha) .. " " .. (c.subject or ""), size = "sm", mono = true, color = "text-soft", truncate = true })
      end
      table.insert(items, column{ gap = 2, w = "full", key = "wt-thread-" .. t.name,
        row{ gap = 8, w = "full", text{ "⑂ " .. t.name, color = "text", truncate = true, grow = 1 },
          text{ table.concat(bits, " · "), size = "sm", color = (t.dirty or 0) > 0 and "amber" or "text-muted" },
          button{ icon = "terminal", variant = "ghost", on = "open", arg = { sandbox = sb, thread = t.name }, key = "wt-open-" .. t.name, help = "Show " .. t.name } },
        text{ t.branch .. " · " .. (t.worktree or ""), size = "xs", color = "text-dim", truncate = true },
        column{ gap = 2, w = "full", table.unpack(commits) },
      })
    end
    table.insert(out, column(items))
  end
  return column(out)
end

local marks = { decision = { "◆", "Decision" }, warning = { "▲", "Warning" }, question = { "?", "Question" }, commit = { "●", "Commit" },
  conflict = { "✕", "Conflict" }, handoff = { "→", "Handoff" }, claim = { "⌖", "Claim" }, release = { "○", "Release" } }
local filter_kinds = { "decision", "warning", "question", "commit" }

local function minutes(ms)
  local m = math.floor((ms or 0) / 60000 + 0.5)
  if m < 60 then return m .. " min" end
  return math.floor(m / 60) .. " h " .. string.format("%02d", m % 60)
end

local function board_tab(state, data, sb)
  local b = (data.board or {})[sb]
  local err = (data.app.errors or {}).board
  if type(err) == "table" then err = err[sb] end
  local out = { gap = 8, w = "full" }
  if err then table.insert(out, note("Could not read the board: " .. err, "red")) end
  if not b or not b.plan or (b.plan.variant ~= "shared") then
    table.insert(out, note("No shared plan at work in " .. sb .. ": the board is where a shared plan's threads claim files and tell each other what they decided."))
    return column(out)
  end
  local claims, entries = b.claims or {}, b.entries or {}
  local open_q = 0
  for _, q in ipairs(entries) do
    if q.kind == "question" then
      local answered = false
      for _, e in ipairs(entries) do
        if (e.at or 0) > (q.at or 0) and (not q.to or e.by == q.to) and (e.kind == "decision" or e.kind == "handoff" or e.kind == "warning") then answered = true end
      end
      if not answered then open_q = open_q + 1 end
    end
  end
  table.insert(out, text{ plural(#claims, "file") .. " claimed · " .. plural(open_q, "open question"), color = "text-bright" })
  table.insert(out, section("Claims", #claims))
  if #claims == 0 then table.insert(out, note("No file is claimed.")) end
  for i, c in ipairs(claims) do
    local conflict = false
    for _, e in ipairs(entries) do
      if e.kind == "conflict" and (e.at or 0) >= (c.since or 0) then
        for _, f in ipairs(e.files or {}) do if f == c.file then conflict = true end end
      end
    end
    local long = (c.held_ms or 0) >= 90 * 60 * 1000
    table.insert(out, column{ gap = 2, w = "full", key = "claim-" .. i,
      row{ gap = 8, w = "full",
        dot{ tone = conflict and "red" or (long and "amber" or "blue") },
        text{ c.file, mono = true, size = "sm", truncate = true, grow = 1, help = c.file },
        (conflict or long) and badge{ conflict and "conflict" or "held long", tone = conflict and "red" or "amber" } or false },
      text{ c.owner .. " · held " .. minutes(c.held_ms) .. ((c.extensions or 0) > 0 and (" · extended " .. c.extensions .. "×") or ""), size = "xs", color = "text-muted" },
    })
  end
  -- The feed, newest first, narrowed by thread and kind (claims always all show).
  local who = {}
  local whoset = {}
  for _, c in ipairs(claims) do if not whoset[c.owner] then whoset[c.owner] = true; table.insert(who, c.owner) end end
  for _, e in ipairs(entries) do if e.by and not whoset[e.by] then whoset[e.by] = true; table.insert(who, e.by) end end
  table.sort(who)
  local ft, fk = state.board_thread or "", state.board_kind or ""
  local titems = { { id = "", label = "Everyone" } }
  for _, w in ipairs(who) do table.insert(titems, { id = w, label = w }) end
  local kitems = { { id = "", label = "All" } }
  for _, k in ipairs(filter_kinds) do table.insert(kitems, { id = k, label = marks[k][2] .. "s" }) end
  table.insert(out, section("Feed", #entries))
  table.insert(out, row{ gap = 8, w = "full",
    select{ id = "board-thread", items = titems, value = ft, on = "board_thread" },
    select{ id = "board-kind", items = kitems, value = fk, on = "board_kind" } })
  local feed = {}
  for _, e in ipairs(entries) do
    if (ft == "" or e.by == ft or e.to == ft) and (fk == "" or e.kind == fk) then table.insert(feed, e) end
  end
  table.sort(feed, function(x, y) return (x.at or 0) > (y.at or 0) end)
  if #feed == 0 then table.insert(out, note("Nothing on the board matches.")) end
  for i, e in ipairs(feed) do
    if i > 60 then break end
    local m = marks[e.kind] or { "•", e.kind }
    local files = {}
    for _, f in ipairs(e.files or {}) do table.insert(files, f) end
    table.insert(out, column{ gap = 2, w = "full", key = "entry-" .. (e.id or i),
      row{ gap = 8, w = "full", text{ m[1], color = e.kind == "conflict" and "red" or (e.kind == "warning" and "amber" or "text-muted") },
        text{ m[2], weight = "medium", size = "sm" },
        text{ e.to and (e.by .. " → " .. e.to) or (e.by or ""), size = "sm", color = "text-muted", truncate = true, grow = 1 },
        e.hash and text{ short(e.hash), mono = true, size = "xs", color = "text-dim" } or false },
      (e.text and e.text ~= "") and text{ e.text, size = "sm", wrap = true } or false,
      #files > 0 and text{ table.concat(files, ", "), size = "xs", mono = true, color = "text-dim", wrap = true } or false,
    })
  end
  return column(out)
end

return {
  init = function() return { board_thread = "", board_kind = "" } end,

  render = function(state, data)
    local sb, sbx = shown(data)
    local ui = data.threads_ui or {}
    local tab = ui.tab or "sessions"
    if not sb then
      return column{ h = "full", pad = 12, gap = 8, note("No sandbox to show: create one, or start one, first.") }
    end
    local items = {}
    for _, s in ipairs(data.sandboxes or {}) do table.insert(items, { id = s.name, label = s.name .. (s.status == "running" and "" or " (" .. s.status .. ")") }) end
    -- Quiet tabs, as Setup's: the current one on a neutral tint, the others
    -- plain. A tab is where you look, not the view's primary action.
    local trow = { gap = 4, w = "full" }
    for _, t in ipairs(tabs) do
      local on_it = t[1] == tab
      table.insert(trow, button{ t[2], variant = on_it and "neutral" or "ghost", color = on_it and "text-bright" or "text-muted",
        on = "tab", arg = { tab = t[1], sandbox = sb }, key = "tab-" .. t[1] })
    end
    local body
    if tab == "worktrees" then body = worktrees_tab(data, sb)
    elseif tab == "board" then body = board_tab(state, data, sb)
    else body = sessions_tab(data, sb, sbx) end
    return column{ h = "full", gap = 8, pad = 12,
      -- The compact sandbox picker (its colour square, then the select), as
      -- the explorer and the configure panels draw it.
      row{ gap = 8, w = "full", text{ "Sandbox", size = "sm", color = "text-muted" },
        sandbox_picker(data, { id = "threads-sandbox", items = items, value = sb, on = "pick" }) },
      row(trow),
      divider{},
      scroll{ gap = 8, body },
    }
  end,

  actions = {
    pick = function(_, _, name) return intent("threads_show", { sandbox = name }) end,
    tab = function(_, a) return intent("threads_show", { sandbox = a.sandbox, tab = a.tab }) end,
    open = function(_, a) return intent("open_terminal", { sandbox = a.sandbox, thread = a.thread, pane = "spare" }) end,
    bash = function(_, a) return intent("thread_bash", { sandbox = a.sandbox, thread = a.thread }) end,
    interrupt = function(_, a) return intent("session_interrupt", { sandbox = a.sandbox, thread = a.thread }) end,
    remote = function(_, a) return intent("remote_control", { sandbox = a.sandbox, thread = a.thread }) end,
    close = function(_, a) return intent("thread_close", { sandbox = a.sandbox, thread = a.thread }) end,
    new = function(_, a) return intent("thread_new", { sandbox = a.sandbox, kind = a.kind }) end,
    review = function(_, id) return intent("plan_review", { id = id }) end,
    lead_start = function(_, sb) return intent("lead_start", { sandbox = sb }) end,
    lead_end = function(_, sb) return intent("lead_end", { sandbox = sb }) end,
    wt_load = function(_, sb) return intent("worktrees_load", { sandbox = sb }) end,
    board_thread = function(state, _, v) state.board_thread = v end,
    board_kind = function(state, _, v) state.board_kind = v end,
  },
}
