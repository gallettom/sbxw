-- New thread (modal, `data.app.modal.arg` = the sandbox), the web UI's
-- thread dialog (panes.js `openThread`): a name, the repository it works in,
-- the model; or, `kind = session`, a session on the main checkout, no
-- worktree. Existing ones of that kind are offered as chips: reopening one
-- is the common case. Open returns `thread_open`, checked again by the core.

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function valid(n)
  if n == "" or #n > 40 then return false end
  if not string.match(n, "^[a-z0-9%-]+$") then return false end
  return string.sub(n, 1, 1) ~= "-" and string.sub(n, -1) ~= "-"
end

local function repo_label(path, ws)
  if path and path ~= "" then return path end
  return (ws or ""):match("([^/]+)/?$") or "workspace"
end

local function default_repo(repos)
  if #repos == 1 then return repos[1].path end
  for _, r in ipairs(repos) do if r.path == "" then return "" end end
  return nil
end

-- What render worked out, for the actions (they do not get `data`).
local form = {}

return {
  init = function() return { name = nil, repo = nil, model = nil, for_key = "" } end,

  render = function(state, data)
    local ui = data.threads_ui or {}
    local new = ui.new or {}
    local sb = (data.app.modal and data.app.modal.arg) or new.sandbox or ""
    local kind = new.kind or "thread"
    local session = kind == "session"
    -- A new dialog starts from its suggestion again.
    local fk = sb .. "/" .. kind .. "/" .. (new.suggest or "")
    local name = state.name
    if state.for_key ~= fk then name = new.suggest or "" end
    local v = (data.threads_view or {})[sb] or {}
    local existing, other = {}, {}
    for _, s in ipairs(v.sessions or {}) do
      if s.thread then
        if (s.kind == "same") == session then table.insert(existing, s) else other[s.thread] = true end
      end
    end
    local known = nil
    for _, s in ipairs(existing) do if s.thread == name then known = s end end
    local repos = (ui.repos_for == sb) and data.thread_repos or nil
    local list = (repos and repos.ok ~= false and repos.repos) or {}
    local repo = state.repo
    if state.for_key ~= fk or repo == nil then repo = repos and repos.ok ~= false and default_repo(list) or nil end
    local models = data.models or {}
    local model = state.model
    if state.for_key ~= fk or model == nil then model = models.default end

    local err = nil
    if name ~= "" and not valid(name) then err = "Lower-case letters, digits and inner hyphens only"
    elseif other[name] then err = session and (name .. " is a thread with a worktree of its own") or (name .. " is a session on the main checkout") end
    local needs_repo = not session and not known and repos ~= nil and repos.ok ~= false and repo == nil and #list > 0
    local no_repo = not session and not known and repos ~= nil and repos.ok ~= false and #list == 0
    local full = v.max ~= nil and (v.open or 0) >= v.max and not (known and known.running)
    form = { sandbox = sb, session = session, name = name, repo = repo, model = model, fresh = known == nil, fk = fk }

    local out = { gap = 12, pad = 16, w = "full" }
    table.insert(out, row{ gap = 8, w = "full",
      text{ session and ("New session in " .. sb .. ", on the main checkout") or ("New thread in " .. sb), size = "xl", weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
      button{ icon = "close", variant = "ghost", on = "cancel", help = "Close", key = "tn-x" } })
    if #existing > 0 then
      local chips = { gap = 8, w = "full" }
      for _, s in ipairs(existing) do
        table.insert(chips, button{ s.thread, variant = s.running and "primary" or "neutral", on = "chip", arg = s.thread, key = "tn-chip-" .. s.thread,
          help = s.running and "Open: go to it" or (session and "Start it again" or "Worktree kept: reopen it") })
      end
      table.insert(out, text{ session and "Existing sessions" or "Existing threads", size = "sm", color = "text-muted" })
      table.insert(out, row(chips))
    end
    table.insert(out, text{ session and "Session name" or "Thread name", size = "sm", color = "text-muted" })
    table.insert(out, input{ id = "tn-name", value = name, placeholder = "t1", on = "name", on_submit = "submit" })
    table.insert(out, note(session
      and "Lower-case letters, digits and hyphens. It works on the same files as the main session (no worktree, no branch), so two agents editing one file can overwrite each other."
      or "Lower-case letters, digits and hyphens. It gets the worktree .sbxw-worktrees/<name> and the branch sbxw/<name>."))
    if err then table.insert(out, text{ err, size = "sm", color = "red", key = "tn-error" }) end
    if not session and not known then
      table.insert(out, text{ "Repository", size = "sm", color = "text-muted" })
      local rerr = (data.app.errors or {}).thread_repos
      if repos == nil and not rerr then
        table.insert(out, note("Reading the repositories…"))
      elseif rerr or (repos and repos.ok == false) then
        table.insert(out, note("Could not list the repositories: " .. (rerr or repos.error or "no answer") .. ". The thread opens in the workspace's default.", "amber"))
      elseif no_repo then
        table.insert(out, note(repo_label("", repos.workspace) .. " holds no git repository. A thread works in a worktree of one: run git init there, or clone a repository into it, then come back.", "amber"))
      else
        local rows = { gap = 4, w = "full" }
        for i, r in ipairs(list) do
          local st = ((r.dirty or 0) > 0) and (r.dirty .. " uncommitted") or "clean"
          if (r.ahead or 0) > 0 or (r.behind or 0) > 0 then st = st .. "  ↑" .. (r.ahead or 0) .. " ↓" .. (r.behind or 0) end
          table.insert(rows, row{ gap = 8, w = "full", pad = 8, radius = "sm", on = "repo", arg = r.path, key = "tn-repo-" .. i,
            fill = r.path == repo and "blue-tint" or "surface-alt", border = r.path == repo and "blue" or "elevated",
            icon{ r.path == repo and "check" or "folder", size = 12, color = r.path == repo and "blue" or "text-muted" },
            text{ repo_label(r.path, repos.workspace) .. (r.path == "" and " (workspace)" or ""), truncate = true, grow = 1 },
            text{ (r.detached and "detached" or (r.branch or "")), size = "sm", color = "text-muted" },
            text{ st, size = "sm", color = (((r.dirty or 0) > 0) or r.detached) and "amber" or "green" } })
        end
        table.insert(out, column(rows))
        if needs_repo then table.insert(out, note("Pick the repository this thread works in.", "amber")) end
      end
    end
    if not known and models.ok and #(models.models or {}) > 0 then
      local items = {}
      for _, m in ipairs(models.models) do table.insert(items, { id = m.id, label = m.label .. (m.id == models.default and " (default)" or "") }) end
      table.insert(out, row{ gap = 8, w = "full", text{ "Model", size = "sm", color = "text-muted" }, select{ id = "tn-model", items = items, value = model or "", on = "model" } })
    end
    if v.max ~= nil then
      table.insert(out, note(tostring(v.open or 0) .. " of " .. tostring(v.max) .. " Claude sessions open" ..
        (full and ": no place is free. Close a thread you no longer need first, or raise max_agent_sessions in sbxw.toml." or "."), full and "amber" or nil))
    end
    local noun = session and "session" or "thread"
    local label = known and (known.running and ("Go to " .. noun) or ("Reopen " .. noun)) or ("Open " .. noun)
    table.insert(out, row{ gap = 8, w = "full", spacer{},
      button{ "Cancel", on = "cancel", key = "tn-cancel" },
      button{ label, variant = "primary", on = "submit", key = "tn-open", disabled = not valid(name) or err ~= nil or needs_repo or no_repo or full } })
    return column(out)
  end,

  actions = {
    cancel = function() return intent("close_modal", {}) end,
    name = function(state, _, v) state.name = v; state.for_key = form.fk; state.repo = form.repo; state.model = form.model end,
    repo = function(state, path) state.repo = path; state.for_key = form.fk; state.name = form.name; state.model = form.model end,
    model = function(state, _, id) state.model = id; state.for_key = form.fk; state.name = form.name; state.repo = form.repo end,
    chip = function(state, name)
      state.for_key = ""
      return intent("thread_open", { sandbox = form.sandbox, name = name, checkout = form.session })
    end,
    submit = function(state)
      local args = { sandbox = form.sandbox, name = form.name, checkout = form.session }
      if form.fresh and not form.session and form.repo ~= nil then args.repo = "=" .. form.repo end
      if form.fresh and form.model then args.model = form.model end
      state.for_key = ""
      return intent("thread_open", args)
    end,
  },
}
