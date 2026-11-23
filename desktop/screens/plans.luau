-- The plan dialog (modal, `data.app.modal.arg` = the plan's id), the web
-- UI's plans.js: a plan the lead proposes, its threads with goal, scope,
-- repository and model, editable before approval (remove, add, edit text,
-- change the variant); then its threads' state and reports once approved;
-- then the merge the lead proposes, with its order and note, and how it went.
--
-- The draft is the core's (`data.threads_ui.drafts[id]`): every edit is an
-- intent, and Approve and Merge only open the core's own dialog, which lists
-- what this file cannot change.

local hues = { "group-1", "group-2", "group-3", "group-4" }
local state_marks = { starting = "…", working = "●", blocked = "?", done = "✓", failed = "✕" }
local state_tones = { starting = "blue", working = "green", blocked = "amber", done = "blue", failed = "red" }
local state_words = { starting = "starting", working = "working", blocked = "wants you", done = "done", failed = "failed" }

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function plural(n, one, many) return tostring(n) .. " " .. (n == 1 and one or (many or one .. "s")) end

local function find_plan(data, id)
  for _, p in ipairs(data.plans or {}) do if p.id == id then return p end end
  return nil
end

-- A goal's first lines and the rest: the first sentence, or (a long one, or
-- none) the first ~160 characters cut at a space, so every card's goal is
-- collapsed the same way with "Show more".
local GOAL_SHOWN = 160
local function first_sentence(goal)
  goal = goal or ""
  local s, e = string.find(goal, "[%.!%?]%s+[A-Z%(\"'`]")
  if s and s <= GOAL_SHOWN + 40 then return string.sub(goal, 1, s), string.sub(goal, e) end
  if #goal <= GOAL_SHOWN + 40 then return goal, "" end
  local cut = GOAL_SHOWN
  while cut > 80 and string.sub(goal, cut, cut) ~= " " do cut = cut - 1 end
  return string.sub(goal, 1, cut - 1) .. "…", string.sub(goal, cut + 1)
end

-- A scope chip's text: whole when short; a long one (a scope written as a
-- sentence) cut in its middle, its full text in the tooltip.
local CHIP_MAX = 44
local function chip_text(s)
  if utf8.len(s) == nil or utf8.len(s) <= CHIP_MAX then return s end
  local head = string.sub(s, 1, utf8.offset(s, 21) - 1)
  local tail = string.sub(s, utf8.offset(s, -20))
  return head .. "…" .. tail
end

local function scope_items(scope)
  local out = {}
  for item in string.gmatch(scope or "", "[^,;\n]+") do
    local t = item:match("^%s*(.-)%s*$")
    if t ~= "" then table.insert(out, t) end
  end
  return out
end

local function repo_label(path, ws)
  if path and path ~= "" then return path end
  return (ws or ""):match("([^/]+)/?$") or "workspace"
end

local function model_label(data, id)
  for _, m in ipairs(((data.models or {}).models) or {}) do if m.id == id then return m.label end end
  return id
end

local function default_repo(repos)
  if #repos == 1 then return repos[1].path end
  for _, r in ipairs(repos) do if r.path == "" then return "" end end
  return nil
end

local function header(title, sub)
  return column{ gap = 4, w = "full",
    row{ gap = 8, w = "full", text{ title, size = "xl", weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
      button{ icon = "close", variant = "ghost", on = "later", help = "Close", key = "plan-x" } },
    (sub and sub ~= "") and markdown{ sub, color = "text-soft" } or false,
  }
end

-- ── Review ──────────────────────────────────────────────────────────────

local function card(state, data, p, t, i, repos)
  local id = p.id
  local key = "pt-" .. i
  local editing = (state.editing or {})[id .. ":" .. i]
  local hue = hues[((i - 1) % 4) + 1]
  local items = { gap = 8, w = "full", pad = 12, radius = "md", fill = "surface-alt", border = hue, key = key }
  table.insert(items, row{ gap = 8, w = "full",
    text{ (t.name ~= "" and t.name) or "Unnamed thread", weight = "semibold", color = t.name ~= "" and "text-bright" or "text-muted", truncate = true, grow = 1 },
    button{ editing and "Done" or "Edit", variant = "ghost", on = "edit_toggle", arg = { id = id, i = i }, key = key .. "-edit" },
    button{ icon = "close", variant = "ghost", on = "remove", arg = { id = id, i = i }, help = "Remove this thread from the plan", key = key .. "-remove" },
  })
  local ws = repos and repos.workspace
  local list = (repos and repos.ok ~= false and repos.repos) or {}
  local needs_repo = t.repo == nil and #list > 1 and default_repo(list) == nil
  local function repo_select()
    local ritems = {}
    if t.repo == nil then
      local d = default_repo(list)
      table.insert(ritems, { id = "", label = needs_repo and "Choose a repository…" or ("Default: " .. repo_label(d, ws)) })
    end
    for _, r in ipairs(list) do
      table.insert(ritems, { id = "=" .. r.path, label = repo_label(r.path, ws) .. "  (" .. ((r.detached and "detached") or (r.branch or "")) .. ", " .. (((r.dirty or 0) > 0) and (r.dirty .. " uncommitted") or "clean") .. ")" })
    end
    return select{ id = key .. "-repo", items = ritems, value = t.repo and ("=" .. t.repo) or "", on = "set_repo", arg = { id = id, i = i } }
  end
  if editing then
    table.insert(items, text{ "Name", size = "sm", color = "text-muted" })
    table.insert(items, input{ id = key .. "-name", value = t.name or "", placeholder = "name", on = "set", arg = { id = id, i = i, field = "name" } })
    table.insert(items, text{ "Goal", size = "sm", color = "text-muted" })
    table.insert(items, input{ id = key .. "-goal", value = t.goal or "", placeholder = "What this thread must do", on = "set", arg = { id = id, i = i, field = "goal" } })
    table.insert(items, text{ "Owns", size = "sm", color = "text-muted" })
    table.insert(items, input{ id = key .. "-scope", value = t.scope or "", placeholder = "Files or areas only this thread edits, separated by commas", on = "set", arg = { id = id, i = i, field = "scope" } })
    if #list > 0 then
      table.insert(items, text{ "Repository", size = "sm", color = "text-muted" })
      table.insert(items, repo_select())
    end
  else
    if t.repo ~= nil then
      table.insert(items, row{ gap = 8, text{ "In", size = "sm", color = "text-muted" }, badge{ repo_label(t.repo, ws), tone = "neutral" } })
    elseif needs_repo then
      -- Asked on the card itself: the one thing that holds Approve.
      table.insert(items, row{ gap = 8, w = "full", text{ "No repository yet:", size = "sm", color = "amber" }, repo_select() })
    end
    local lead, rest = first_sentence(t.goal)
    table.insert(items, text{ lead ~= "" and lead or "No goal yet", color = lead ~= "" and "text" or "text-muted", wrap = true })
    if rest ~= "" then
      local open = (state.more or {})[id .. ":" .. i]
      if open then table.insert(items, text{ rest, size = "sm", color = "text-soft", wrap = true }) end
      table.insert(items, button{ open and "Show less" or "Show more", variant = "ghost", on = "more", arg = { id = id, i = i }, key = key .. "-more" })
    end
    -- What it owns: chips on as many lines as they need, inside the card;
    -- past 12, a "+N more" that opens the rest.
    local chips = { gap = 4, w = "full", wrap = true, key = key .. "-owns", text{ "Owns", size = "sm", color = "text-muted" } }
    local sc = scope_items(t.scope)
    if #sc == 0 then table.insert(chips, text{ "Nothing named", size = "sm", color = "text-muted" }) end
    local all = (state.owns or {})[id .. ":" .. i]
    for k, s in ipairs(sc) do
      if k > 12 and not all then
        table.insert(chips, button{ "+" .. (#sc - 12) .. " more", variant = "ghost", dense = true, on = "owns", arg = { id = id, i = i }, key = key .. "-owns-more" })
        break
      end
      local shown = chip_text(s)
      table.insert(chips, row{ key = key .. "-scope-" .. k, help = shown ~= s and s or nil, badge{ shown, tone = "neutral" } })
    end
    table.insert(items, row(chips))
  end
  local models = data.models or {}
  if models.ok and #(models.models or {}) > 0 then
    local mitems = { { id = "", label = "Default (" .. model_label(data, models.default) .. ")" } }
    local known = false
    for _, m in ipairs(models.models) do table.insert(mitems, { id = m.id, label = m.label }); if m.id == t.model then known = true end end
    if t.model and not known then table.insert(mitems, { id = t.model, label = t.model }) end
    table.insert(items, row{ gap = 8, w = "full", text{ "Model", size = "sm", color = "text-muted" },
      select{ id = key .. "-model", items = mitems, value = t.model or "", on = "set_model", arg = { id = id, i = i } } })
  end
  return column(items), needs_repo
end

local function review(state, data, p)
  local ui = data.threads_ui or {}
  local draft = (ui.drafts or {})[p.id] or { threads = p.threads, variant = p.variant }
  local repos = (ui.repos_for == p.sandbox) and data.thread_repos or nil
  local v = (data.threads_view or {})[p.sandbox] or {}
  local out = { gap = 12, w = "full", h = "full" }
  -- A short window (under 600 pt): only the title stays fixed; the summary,
  -- the variant and the counts scroll with the threads, so the cards keep room.
  local short = (((data.app or {}).window or {}).h or 900) < 600
  local fixed = out
  local cards = { gap = 8, w = "full" }
  if short then
    table.insert(out, header("Plan from the lead of " .. p.sandbox, nil))
    out = cards
    if p.summary and p.summary ~= "" then table.insert(out, markdown{ p.summary, color = "text-soft" }) end
  else
    table.insert(out, header("Plan from the lead of " .. p.sandbox, p.summary))
  end
  -- The variant, as the lead proposed it with its reason; the choice here is what Approve sends.
  local variant = draft.variant or "split"
  table.insert(out, row{ gap = 8, w = "full",
    text{ "Variant", size = "sm", color = "text-muted" },
    select{ id = "plan-variant", value = variant, on = "variant", arg = p.id, items = {
      { id = "split", label = "Split: a worktree and a branch per thread" },
      { id = "shared", label = "Shared: one worktree, one branch, files claimed on a board" } } },
  })
  if p.justification and p.justification ~= "" then
    table.insert(out, row{ gap = 8, w = "full", text{ "Why", size = "sm", color = "text-muted" }, text{ p.justification, size = "sm", wrap = true, grow = 1 } })
  end
  local n = #(draft.threads or {})
  local open, max = v.open, v.max
  local running = {}
  for _, s in ipairs(v.sessions or {}) do if s.running then running[s.thread or "main"] = true end end
  local new = 0
  for _, t in ipairs(draft.threads or {}) do if not running[t.name] then new = new + 1 end end
  local line = plural(n, "thread")
  if max then line = line .. " · " .. tostring(open or 0) .. " of " .. tostring(max) .. " Claude sessions open now" end
  table.insert(out, text{ line, size = "sm", color = "text-muted", key = "plan-count" })
  if max and (open or 0) + new > max then
    local free = math.max(0, max - (open or 0))
    table.insert(out, note("Too many for now: these threads need " .. plural(new, "new session") .. " and " .. p.sandbox .. " has " ..
      (free == 1 and "1 place" or (free .. " places")) .. " free (max_agent_sessions = " .. max .. "). Remove " .. plural(new - free, "thread") ..
      ", close a thread in the Threads panel, or raise max_agent_sessions in sbxw.toml.", "amber"))
  end
  if repos and repos.ok ~= false and #(repos.repos or {}) == 0 then
    table.insert(out, note(repo_label("", repos.workspace) .. " holds no git repository, and every thread needs one: run git init there, or clone one into it.", "amber"))
  end
  out = fixed
  local missing = 0
  for i, t in ipairs(draft.threads or {}) do
    local c, needs = card(state, data, p, t, i, repos)
    if needs then missing = missing + 1 end
    table.insert(cards, c)
  end
  if n == 0 then table.insert(cards, note("No thread left: add one, or refuse the plan.")) end
  table.insert(cards, row{ gap = 8, button{ "Add thread", icon = "plus", on = "add", arg = p.id, key = "plan-add" },
    button{ "Undo my changes", variant = "ghost", on = "reset", arg = p.id, key = "plan-reset" } })
  -- The threads are the part that scrolls, between the fixed header and
  -- footer; its edges fade where cards are hidden.
  table.insert(out, scroll{ gap = 8, grow = 1, fade = 24, key = "plan-cards", column(cards) })
  local err = (ui.plan_error or {})[p.id]
  if err then table.insert(out, text{ err, color = "red", wrap = true, key = "plan-error" }) end
  if missing > 0 then table.insert(out, note("Choose a repository for " .. plural(missing, "thread") .. " first.", "amber")) end
  table.insert(out, row{ gap = 8, w = "full",
    input{ id = "plan-note", value = state.note or "", placeholder = "Why you refuse (optional, the lead reads it)", on = "note_text", grow = 1 },
    button{ "Refuse", variant = "danger", on = "refuse", arg = p.id, key = "plan-refuse" },
    button{ "Later", on = "later", key = "plan-later" },
    button{ "Approve and start", variant = "primary", on = "approve", arg = p.id, key = "plan-approve", disabled = n == 0 or missing > 0 },
  })
  return column(out)
end

-- ── Status and merge ────────────────────────────────────────────────────

local function status(data, p)
  local items = { gap = 8, w = "full" }
  for i, t in ipairs(p.threads or {}) do
    local st = t.state or "starting"
    local running = false
    local v = (data.threads_view or {})[p.sandbox] or {}
    for _, s in ipairs(v.sessions or {}) do if s.thread == t.name and s.running then running = true end end
    table.insert(items, column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = hues[((i - 1) % 4) + 1], key = "ps-" .. t.name,
      row{ gap = 8, w = "full",
        text{ state_marks[st] or "•", color = state_tones[st] or "text-muted" },
        text{ t.name, weight = "medium", color = "text-bright", truncate = true, grow = 1 },
        badge{ state_words[st] or st, tone = state_tones[st] or "neutral" },
        not running and badge{ "session stopped", tone = "neutral" } or false,
        button{ icon = "terminal", variant = "ghost", on = "open", arg = { sandbox = p.sandbox, thread = t.name }, help = "Show " .. t.name, key = "ps-" .. t.name .. "-open" } },
      text{ first_sentence(t.goal), size = "sm", color = "text-soft", wrap = true },
      (t.summary and t.summary ~= "") and text{ "Report: " .. t.summary, size = "sm", wrap = true } or false,
    })
  end
  return column(items)
end

local merge_marks = { ok = { "✓", "green" }, failed = { "✕", "red" }, pending = { "…", "blue" }, skipped = { "–", "text-muted" } }

local function merge_view(data, p)
  local ui = data.threads_ui or {}
  local m = p.merge
  local out = { gap = 8, w = "full" }
  local order = m.order or {}
  if #order == 0 then for _, t in ipairs(p.threads or {}) do table.insert(order, t.name) end end
  local results = {}
  for _, r in ipairs(m.results or {}) do results[r.thread] = r end
  local shared = p.variant == "shared"
  table.insert(out, text{ shared and ("Merge the shared branch sbxw/team-" .. p.id) or "Merge, in the lead's order", weight = "semibold", color = "text-bright" })
  if m.note and m.note ~= "" then table.insert(out, row{ gap = 8, w = "full", text{ "Lead's note", size = "sm", color = "text-muted" }, text{ m.note, size = "sm", wrap = true, grow = 1 } }) end
  local rows = { gap = 4, w = "full" }
  local team_r = nil
  for _, r in ipairs(m.results or {}) do if string.sub(r.thread or "", 1, 5) == "team-" then team_r = r end end
  for i, name in ipairs(order) do
    local r = results[name] or (shared and team_r) or nil
    local st, txt = "", ""
    if r and r.ok then st = "ok"; txt = r.commit and string.sub(r.commit, 1, 7) or "merged"
    elseif r then st = "failed"; txt = r.error or "failed"
    elseif m.state == "merging" then st = "pending"; txt = "pending"
    elseif m.state == "failed" or m.state == "merged" then st = "skipped"; txt = "not merged" end
    local mk = merge_marks[st]
    table.insert(rows, row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = hues[((i - 1) % 4) + 1], key = "mr-" .. name,
      text{ i .. ".", size = "sm", color = "text-muted" },
      text{ name, weight = "medium", truncate = true },
      spacer{},
      mk and text{ mk[1], color = mk[2] } or false,
      txt ~= "" and text{ txt, size = "sm", mono = st == "ok", color = st == "failed" and "red" or "text-muted", wrap = st == "failed", max_w = 420 } or false,
    })
  end
  table.insert(out, column(rows))
  -- The outcome in words.
  if m.state == "merged" then
    -- What each thread's result says (nothing to merge, files left alone), as the web UI's outcome.
    local s = m.base and ("Merged into " .. m.base .. ".") or "Merged."
    for _, r in ipairs(m.results or {}) do if r.ok and r.note and r.note ~= "" then s = s .. " " .. r.thread .. ": " .. r.note .. "." end end
    table.insert(out, text{ s, color = "green", wrap = true, key = "merge-outcome" })
  elseif m.state == "failed" then
    local bad, done = nil, {}
    for _, r in ipairs(m.results or {}) do if r.ok then done[r.thread] = true elseif not bad then bad = r end end
    local rest = {}
    for _, n in ipairs(order) do if not done[n] and (not bad or bad.thread ~= n) then table.insert(rest, n) end end
    local s = bad and ("Stopped at " .. bad.thread .. ": " .. (bad.error or "no reason given") .. ".") or "The merge failed."
    if #rest > 0 then s = s .. " Not merged: " .. table.concat(rest, ", ") .. "." end
    table.insert(out, text{ s, color = "red", wrap = true, key = "merge-outcome" })
  end
  local mn = (ui.merge_note or {})[p.id]
  if mn then table.insert(out, text{ mn, color = string.sub(mn, 1, 4) == "Not " and "amber" or "text-muted", wrap = true, key = "merge-note" }) end
  -- A shared plan: what the board says (claims hold Merge; commits by thread; the distillate).
  if shared then
    local b = (data.board or {})[p.sandbox] or {}
    local claims = b.claims or {}
    if #claims > 0 and m.state ~= "merged" then
      local files = {}
      for _, c in ipairs(claims) do table.insert(files, c.file .. " (" .. c.owner .. ")") end
      table.insert(out, note(plural(#claims, "file") .. " still claimed: Merge waits until they are released. " .. table.concat(files, ", "), "amber"))
    end
    local commits = {}
    for _, e in ipairs(b.entries or {}) do if e.kind == "commit" then commits[e.by] = (commits[e.by] or 0) + 1 end end
    local cl = {}
    for who, k in pairs(commits) do table.insert(cl, who .. " " .. plural(k, "commit")) end
    table.sort(cl)
    table.insert(out, note(#cl > 0 and ("Commits by thread: " .. table.concat(cl, ", ")) or "No commit on the board."))
    local dist = m.distillate or {}
    if #dist > 0 then
      table.insert(out, text{ "Kept in auto memory with the merge", size = "sm", color = "text-muted" })
      for k, d in ipairs(dist) do
        table.insert(out, row{ gap = 8, w = "full", key = "dist-" .. k, text{ d.kind == "warning" and "▲" or "◆", color = d.kind == "warning" and "amber" or "blue" }, text{ d.text, size = "sm", wrap = true, grow = 1 } })
      end
    end
  end
  return column(out)
end

local function approved(state, data, p)
  local ui = data.threads_ui or {}
  local out = { gap = 12, w = "full", h = "full" }
  local over = p.merge and p.merge.state == "merged"
  table.insert(out, header((p.merge and "Merge the threads of " or "Plan of ") .. p.sandbox, p.summary))
  if p.closed then table.insert(out, note("Team mode was ended: the threads run on, their reports no longer reach the lead.")) end
  local body = { gap = 12, w = "full" }
  if p.merge then table.insert(body, merge_view(data, p)) end
  table.insert(body, text{ "Threads", size = "sm", weight = "semibold", color = "text-muted" })
  table.insert(body, status(data, p))
  table.insert(out, scroll{ gap = 8, grow = 1, fade = 24, key = "plan-body", column(body) })
  local err = (ui.plan_error or {})[p.id]
  if err then table.insert(out, text{ err, color = "red", wrap = true, key = "plan-error" }) end
  local buttons = { gap = 8, w = "full" }
  table.insert(buttons, button{ icon = "reload", on = "reload", help = "Read the plan again", key = "plan-reload" })
  table.insert(buttons, spacer{})
  if p.merge and not over then
    local running = p.merge.state == "merging"
    local close = (ui.merge_close or {})[p.id]
    if close == nil then close = true end
    table.insert(buttons, checkbox{ "Close each thread once merged", value = close, on = "close_set", arg = p.id, disabled = running, key = "merge-close" })
    table.insert(buttons, button{ "Not now", on = "later", key = "merge-later" })
    -- A shared plan's claims hold Merge until released (the daemon refuses it anyway).
    local held = p.variant == "shared" and #((((data.board or {})[p.sandbox]) or {}).claims or {}) > 0
    table.insert(buttons, button{ running and "Merging…" or (p.merge.state == "failed" and "Retry merge" or "Merge all"), variant = "primary", on = "merge", arg = p.id, busy = running, disabled = held and not running, key = "merge-all",
      help = held and "Files are still claimed: release them first" or nil })
  else
    table.insert(buttons, button{ "Close", on = "later", key = "plan-close" })
  end
  table.insert(out, row(buttons))
  return column(out)
end

return {
  init = function() return { editing = {}, more = {}, note = "" } end,

  render = function(state, data)
    local id = data.app.modal and data.app.modal.arg
    local p = id and find_plan(data, id)
    if not p then
      return column{ gap = 12, pad = 16, w = "full", header("Plan"), note(((data.app.errors or {}).plans and ("Could not read the plans: " .. data.app.errors.plans)) or "This plan is gone: its lead may have been replaced."),
        row{ spacer{}, button{ "Close", on = "later", key = "plan-close" } } }
    end
    local body
    if p.state == "pending" and not p.closed then body = review(state, data, p)
    elseif p.state == "approved" then body = approved(state, data, p)
    else
      body = column{ gap = 12, w = "full", header("Plan of " .. p.sandbox, p.summary),
        note("Refused" .. ((p.note and p.note ~= "") and (": " .. p.note) or ".") .. (p.closed and " Team mode was ended." or "")),
        row{ spacer{}, button{ "Close", on = "later", key = "plan-close" } } }
    end
    -- As tall as the dialog the core gives it: the footer at its bottom edge.
    return column{ pad = 16, gap = 12, w = "full", h = "full", body }
  end,

  actions = {
    later = function() return intent("close_modal", {}) end,
    reload = function() return intent("plans_reload", {}) end,
    edit_toggle = function(state, a)
      state.editing = state.editing or {}
      local k = a.id .. ":" .. a.i
      state.editing[k] = not state.editing[k]
    end,
    owns = function(state, a)
      state.owns = state.owns or {}
      local k = a.id .. ":" .. a.i
      state.owns[k] = not state.owns[k]
    end,
    more = function(state, a)
      state.more = state.more or {}
      local k = a.id .. ":" .. a.i
      state.more[k] = not state.more[k]
    end,
    remove = function(state, a)
      state.editing = {}
      return intent("plan_remove", { id = a.id, index = a.i })
    end,
    add = function(state, id)
      return intent("plan_add", { id = id })
    end,
    reset = function(state, id) state.editing = {}; return intent("plan_reset", { id = id }) end,
    set = function(_, a, v)
      if v == nil or v == "" then return intent("plan_edit", { id = a.id, index = a.i, field = a.field }) end
      return intent("plan_edit", { id = a.id, index = a.i, field = a.field, value = v })
    end,
    set_repo = function(_, a, v)
      if v == "" then return intent("plan_edit", { id = a.id, index = a.i, field = "repo" }) end
      return intent("plan_edit", { id = a.id, index = a.i, field = "repo", value = v })
    end,
    set_model = function(_, a, v)
      if v == "" then return intent("plan_edit", { id = a.id, index = a.i, field = "model" }) end
      return intent("plan_edit", { id = a.id, index = a.i, field = "model", value = v })
    end,
    variant = function(_, id, v) return intent("plan_variant", { id = id, variant = v }) end,
    note_text = function(state, _, v) state.note = v end,
    refuse = function(state, id)
      local n = state.note
      state.note = ""
      if n and n ~= "" then return intent("plan_refuse", { id = id, note = n }) end
      return intent("plan_refuse", { id = id })
    end,
    approve = function(_, id) return intent("plan_approve", { id = id }) end,
    merge = function(_, id) return intent("plan_merge", { id = id }) end,
    close_set = function(_, id, on) return intent("merge_close_set", { id = id, on = on }) end,
    open = function(_, a) return intent("open_terminal", { sandbox = a.sandbox, thread = a.thread, pane = "spare" }) end,
  },
}
