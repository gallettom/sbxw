-- Commits (an overlay over the pane area, feat/commits.rs): the latest
-- commits of every sandbox's workspace, newest first. Read only: the view
-- shows, filters, opens a commit in place and copies a hash; nothing here
-- runs git.
--
-- Commit messages and paths come from git, written by people and agents:
-- they are drawn as plain text (no pills, no Markdown, no links).

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function filters(ui, V)
  local items = { { id = "", label = "All sandboxes", icon = "grid" } }
  for _, s in ipairs(ui.sandboxes or {}) do items[#items + 1] = { id = s.id, label = s.label } end
  return row{ gap = 8, w = "full",
    V == "" and select{ id = "commits-sandbox", value = ui.sandbox or "", on = "sandbox", items = items, w = 180 } or false,
    -- The field grows and never goes under its placeholder; when the row
    -- has no room for it, the row wraps (the controls keep their width).
    input{ id = "commits-filter", value = ui.filter or "", placeholder = "Filter by subject or author", on = "filter", grow = 1, min_w = 220 },
    checkbox{ "Only unmerged", value = ui.unmerged == true, on = "unmerged", key = "commits-unmerged" },
    nz(ui.branch) and row{ gap = 4, key = "commits-branch", px = 8, h = 24, radius = "pill", fill = "blue-tint",
      icon{ "branch", size = 12, color = "blue" },
      text{ ui.branch, size = "caption", color = "text-bright" },
      button{ icon = "close", key = "commits-branch-clear", on = "branch_clear", variant = "ghost", dense = true, help = "Every branch again" },
    } or false,
    button{ icon = "reload", key = "commits-refresh", on = "refresh", variant = "ghost", help = "Read the commits again" },
  }
end

-- A file of the open commit: its counts, pressed for its diff under it.
local function fileview(f)
  local kids = { gap = 4, w = "full",
    row{ gap = 8, w = "full", px = 8, py = 4, radius = "xs", key = "commits-file-" .. f.path, on = "file", arg = f.path, hover = "hover-subtle",
      help = f.open and "Hide its diff" or "Show its diff",
      icon{ f.open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
      icon{ "file", size = 12, color = "text-muted" },
      text{ f.path, size = "sm", mono = true, truncate = true, grow = 1 },
      f.binary and text{ "binary", size = "xs", color = "text-muted" } or false,
      f.added ~= nil and text{ "+" .. tostring(f.added), size = "xs", mono = true, color = "green" } or false,
      f.removed ~= nil and text{ "-" .. tostring(f.removed), size = "xs", mono = true, color = "red" } or false,
    },
  }
  if f.open then
    local d = { gap = 0, w = "full", key = "commits-diff-" .. f.path, radius = "xs", fill = "floor", border = "elevated", pad = 4, clip = true }
    for _, g in ipairs(f.groups or {}) do
      local color, fill = "text", nil
      if g.kind == "add" then color, fill = "green", "green-tint"
      elseif g.kind == "del" then color, fill = "red-text", "red-tint"
      elseif g.kind == "hunk" then color = "text-muted" end
      -- Long lines wrap inside the block: the layout never widens.
      d[#d + 1] = column{ w = "full", px = 4, fill = fill, text{ g.text, mono = true, size = "xs", wrap = true, color = color } }
    end
    if nz(f.note) then d[#d + 1] = note(f.note, "amber") end
    if #(f.groups or {}) == 0 and not nz(f.note) then d[#d + 1] = note("No change to show (a mode change or an empty file).") end
    kids[#kids + 1] = column(d)
  elseif nz(f.note) and f.binary then
    -- said on the row already
  end
  return column(kids)
end

local function detailview(d)
  local kids = { gap = 8, w = "full", px = 24, py = 4, key = "commits-detail" }
  if d.loading then
    kids[#kids + 1] = note("Reading the commit…")
  elseif nz(d.error) then
    kids[#kids + 1] = note(d.error, "red")
  else
    if nz(d.body) then kids[#kids + 1] = text{ d.body, size = "sm", wrap = true, color = "text", key = "commits-body" } end
    local n = #(d.files or {})
    kids[#kids + 1] = text{ n .. (n == 1 and " file changed" or " files changed"), size = "caption", weight = "semibold", color = "text-muted" }
    for _, f in ipairs(d.files or {}) do kids[#kids + 1] = fileview(f) end
    if (d.files_more or 0) > 0 then kids[#kids + 1] = note(d.files_more .. " more files not listed: truncated", "amber") end
    if d.truncated then kids[#kids + 1] = note("Part of this commit is truncated: the app shows 64 KB a file, 512 KB in all. git show in a terminal has it whole.") end
  end
  return column(kids)
end

local function rowview(r)
  local branch = nz(r.branch_label) and row{ gap = 4,
    icon{ "branch", size = 12, color = r.thread and "blue" or "text-muted" },
    text{ r.branch_label, size = "xs", color = r.thread and "text-bright" or "text-muted", truncate = true, max_w = 160 },
  } or false
  return row{ gap = 8, w = "full", px = 8, h = 32, radius = "xs", key = "commits-row-" .. r.id, on = "expand", arg = r.id,
    selected = r.selected == true, fill = (r.expanded or r.selected) and "active-bg" or nil, hover_group = true,
    help = r.expanded and "Fold it" or "Open it: its message and changed files",
    icon{ r.expanded and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
    column{ w = 8, h = 8, radius = 2, fill = r.color or "text-dim", help = r.sandbox },
    text{ r.sandbox or "", size = "xs", color = "text-muted", truncate = true, max_w = 120 },
    text{ r.short or "", size = "xs", mono = true, color = "text-muted" },
    text{ r.subject or "", size = "sm", color = "text-bright", truncate = true, grow = 1, min_w = 140, help = r.subject },
    branch,
    r.merged and badge{ "merged", tone = "green", help = "The main checkout's branch holds it" } or false,
    text{ r.author or "", size = "xs", color = "text-muted", truncate = true, max_w = 120 },
    text{ r.when or "", size = "xs", color = "text-dim" },
    button{ icon = "copy", key = "commits-copy-" .. r.id, on = "copy", arg = r.id, variant = "ghost", dense = true, help = "Copy the full hash" },
  }
end

local M = {
  init = function() return {} end,
  render = function(state, data)
    local ui = data.commits_ui or {}
    local V = (data.app or {}).scope_variant or ""
    local list = { gap = 2, w = "full", folds = true, group = "list", key = "commits-list" }
    for _, r in ipairs(ui.rows or {}) do
      list[#list + 1] = rowview(r)
      if r.expanded and ui.detail then list[#list + 1] = detailview(ui.detail) end
    end
    local more = more_rows(ui.count, { key = "commits-more", on = "more", what = "commits", hint = "Filter to find one" })
    if more then list[#list + 1] = more end
    if #(ui.rows or {}) == 0 then
      list[#list + 1] = note(ui.loading and "Reading the commits…" or (nz(ui.filter) and "No commit matches the filter." or "No commits to show."))
    end
    for _, n in ipairs(ui.notes or {}) do
      list[#list + 1] = note(n.text, n.tone == "red" and "red" or "text-dim")
    end
    local out = { h = "full", gap = 8, w = "full" }
    if V == "3" then
      out[#out + 1] = row{ gap = 12, w = "full", key = "scope-bar",
        scope_block(data, { id = "commits-sandbox", value = ui.sandbox or "", open = state.scope_open, narrow = nz(ui.branch) and ("branch " .. ui.branch) or nil, all = "All sandboxes" }),
        column{ grow = 1, min_w = 0, filters(ui, V) } }
    else
      if V ~= "" then
        out[#out + 1] = row{ key = "title-slot", gap = 6,
          text{ "in", size = "lg", color = "text-muted" },
          scope_capsule(data, { id = "commits-sandbox", value = ui.sandbox or "", open = state.scope_open, all = "All sandboxes" }) }
      end
      out[#out + 1] = filters(ui, V)
    end
    out[#out + 1] = scroll{ key = "commits-scroll", column(list) }
    return column(out)
  end,
  actions = {
    sandbox = function(_, _, id) return intent("commits_sandbox", { sandbox = id or "" }) end,
    scope_toggle = function(state) state.scope_open = not state.scope_open end,
    scope_filter = function(state, _, t) state.scope_filter = t end,
    scope_pick = function(state, id) state.scope_open = false; return intent("commits_sandbox", { sandbox = id or "" }) end,
    filter = function(_, _, t) return intent("commits_filter", { text = t or "" }) end,
    unmerged = function(_, _, on) return intent("commits_unmerged", { on = on }) end,
    branch_clear = function() return intent("commits_branch_clear", {}) end,
    refresh = function() return intent("commits_refresh", {}) end,
    expand = function(_, id) return intent("commits_expand", { id = id }) end,
    file = function(_, p) return intent("commits_file", { path = p }) end,
    more = function() return intent("commits_more", {}) end,
    copy = function(_, id) return intent("commits_copy", { id = id }) end,
  },
}
-- STAGE1 THROWAWAY: the scope list on top of everything.
local inner = M.render
M.render = function(state, data)
  local r = inner(state, data)
  local V = (data.app or {}).scope_variant or ""
  local ui = data.commits_ui or {}
  if V == "" or not state.scope_open then return r end
  return column{ h = "full", w = "full", r,
    scope_menu(data, { open = true, value = ui.sandbox or "", all = "All sandboxes", pick = "scope_pick", left = V == "3" and 0 or 104, top = V == "3" and 50 or 0, filter = state.scope_filter, show_filter = #(data.sandboxes or {}) > ((data.app or {}).scope_filter_min or 8) }) }
end
return M
