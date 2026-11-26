-- The Kanban (feat/kanban/): the Dashboard's second view. Five columns side by
-- side (Planned, In progress, In test, Ready to merge, Merged; a daemon with
-- ticket sources puts Backlog first, the tracker's issues), each with its
-- count and its cards stacked and scrolling inside it; under 900 pt one
-- column at a time with a column picker. Pressing a card opens its ticket
-- as a panel over the board (not a second column); Escape, its cross or a
-- press outside closes it.
--
-- Everything here comes ready from Rust (model.rs): each card's column (the
-- daemon's), its texts already clipped, its marks, its element key, the
-- counts. The screen does no string work per card: it picks one of two
-- prepared titles by one comparison of the column's width, and lays out.
--
-- Ticket and agent words are data: drawn as text (the prose face, pills),
-- never as a control. A tracker's words (an issue's title, its key) too: the
-- issue's link opens only through Rust, which keeps the url.

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local VIEWS = { { id = "conversation", label = "Conversation" }, { id = "kanban", label = "Kanban" } }

-- The sandbox beside the overlay's title ("Dashboard in [web-app v]").
local function scope(ui)
  return row{ key = "title-slot", gap = 8,
    select{ id = "kanban-sandbox", value = ui.scope or "", items = ui.sandboxes or {}, on = "scope",
      scope = { kind = "sandbox", all = true } } }
end

local function mark(m)
  if m.quiet then return text{ m.label, size = "xs", color = "text-muted", help = m.help } end
  return badge{ m.label, tone = m.tone or "neutral", help = m.help }
end

local function marks(c)
  local r = { gap = 4, wrap = true, w = "full" }
  for _, m in ipairs(c.marks or {}) do r[#r + 1] = mark(m) end
  return row(r)
end

-- An imported card's issue key, a link to it, and on a Backlog card the one
-- way out: "Move to Planned".
local function origin(c)
  local o = c.origin
  if not o then return false end
  return row{ gap = 4, w = "full", wrap = true,
    o.open and button{ o.key or "", icon = "link", variant = "ghost", dense = true, on = "issue", arg = c.id, key = o.el, help = o.open_help } or
      text{ o.key or "", size = "xs", mono = true, color = "text-muted", help = o.open_help },
    spacer{},
    c.plan and button{ "Move to Planned", variant = "ghost", tone = "blue", dense = true, on = "plan", arg = c.id, key = c.plan.el, help = "Planned is where a lead takes tickets from" } or false }
end

local function card(c, wide)
  return column{ key = c.id, on = "open", arg = c.id, selected = c.selected,
    gap = 4, px = 12, py = 8, radius = "md", w = "full",
    fill = c.selected and "active-bg" or "surface", border = c.border or "elevated", hover = "hover-subtle",
    help = c.help,
    row{ gap = 8, w = "full",
      c.team_color and column{ w = 8, h = 8, radius = "mark", fill = c.team_color, help = c.team_help } or false,
      text{ c.num or "", size = "xs", mono = true, color = "text-muted", help = c.thread_card and "A thread with no ticket: its plan's goal" or nil },
      c.sandbox_label and text{ c.sandbox_label, size = "xs", mono = true, color = "text-muted", truncate = true, max_w = 120 } or false,
      spacer{},
      text{ c.age or "", size = "xs", color = "text-muted" } },
    text{ wide and (c.title_long or "") or (c.title or ""), size = "sm", wrap = true, color = "text-bright" },
    #(c.marks or {}) > 0 and marks(c) or false,
    origin(c),
    c.thread and row{ gap = 8, w = "full",
      dot{ tone = c.thread_tone or "neutral" },
      text{ c.thread, size = "xs", mono = true, color = c.thread_color or "text-muted", truncate = true, grow = 1 } } or false,
  }
end

-- The sandbox's ticket source: its status line and its button. Absent with
-- every sandbox shown, and with a daemon that has no sources.
local function source(s)
  if not s then return false end
  return row{ gap = 8, w = "full", key = "kanban-source", px = 4,
    icon{ "link", size = 12, color = "text-muted" },
    dot{ tone = s.tone or "neutral" },
    text{ s.text or "", size = "xs", color = s.failed and "red" or "text-muted", truncate = true, grow = 1, help = s.help, key = "kanban-source-line" },
    s.on and button{ "Refresh", icon = "reload", variant = "ghost", dense = true, on = "source_refresh", key = "kanban-source-refresh", disabled = s.syncing == true, help = s.syncing and "Syncing now" or "Sync with the tracker now" } or false,
    button{ s.on and "Source…" or "Connect", variant = "ghost", tone = s.on and nil or "blue", dense = true, on = "source", key = "kanban-source-open", help = s.on and "The ticket source: its repository, its label, what sbxw writes there" or "Import a repository's issues as Backlog tickets" } }
end

local function head(col, ui)
  return row{ gap = 8, w = "full", px = 4, h = 28, key = col.el_head,
    dot{ tone = col.tone or "neutral" },
    text{ col.title or "", size = "sm", weight = "semibold", color = "text-bright", truncate = true },
    badge{ tostring(col.count or 0), tone = "neutral", help = col.count_help },
    spacer{},
    col.id == "planned" and button{ icon = "plus", variant = "ghost", dense = true, on = "new", key = "kanban-new-col", help = ui.new_why or "New ticket", disabled = ui.new_why ~= nil, keys = { "N" } } or false }
end

local function lane(col, col_w, ui, narrow)
  local wide = col_w >= 300
  local kids = { gap = 8, w = "full", group = "list" }
  for _, c in ipairs(col.cards or {}) do kids[#kids + 1] = card(c, wide) end
  if #(col.cards or {}) == 0 then
    kids[#kids + 1] = column{ px = 8, py = 12, gap = 8, key = "kanban-empty-" .. col.id, note(col.empty or "Nothing here.", "text-muted"),
      col.id == "planned" and not ui.new_why and button{ "Add a ticket", icon = "plus", variant = "ghost", tone = "blue", dense = true, on = "new", key = "kanban-new-empty" } or false }
  end
  if col.more then
    kids[#kids + 1] = button{ col.more.label, variant = "ghost", tone = "blue", dense = true, on = "more", arg = col.id, key = col.more.el }
  end
  if col.capped then
    kids[#kids + 1] = note("The first cards are drawn: a sandbox of its own shows fewer.", "text-muted")
  end
  if col.older then
    kids[#kids + 1] = button{ col.older.label, variant = "ghost", tone = "blue", dense = true, on = "older", key = col.older.el, help = "Merged cards past the recent ones" }
  end
  return column{ gap = 8, h = "full", grow = narrow and 1 or nil, w = (not narrow) and col_w or "full", pad = 8, radius = "md", fill = "floor", key = col.el_lane,
    head(col, ui),
    scroll{ key = col.el_scroll, fade = 12, column(kids) } }
end

-- The ticket, as a panel over the board.
local function panel(t, ow)
  if not t then return false end
  local w = math.min(640, ow - 48)
  local kids = { gap = 12, w = "full", px = 4 }
  kids[#kids + 1] = row{ gap = 8, w = "full",
    text{ t.num or "", size = "sm", mono = true, color = "text-muted" },
    nz(t.column_label) and badge{ t.column_label, tone = t.column_tone or "neutral" } or false,
    spacer{},
    button{ icon = "note", variant = "ghost", dense = true, on = "edit", key = "kanban-edit", help = t.edit_why or "Edit the title, description and files", disabled = (not t.editable) or t.loading },
    button{ icon = "trash", variant = "ghost", tone = "red", dense = true, on = "delete", key = "kanban-delete", help = t.delete_why, disabled = (not t.deletable) or t.loading },
    button{ icon = "close", variant = "ghost", dense = true, on = "close", key = "kanban-close", help = "Close", keys = { "Esc" } } }
  if #(t.marks or {}) > 0 then
    local m = { gap = 4, wrap = true }
    for _, x in ipairs(t.marks) do m[#m + 1] = mark(x) end
    kids[#kids + 1] = row(m)
  end
  kids[#kids + 1] = text{ t.title or "", size = "lg", weight = "semibold", wrap = true, color = "text-bright", key = "kanban-panel-title" }
  if t.imported then
    local im = t.imported
    kids[#kids + 1] = row{ gap = 8, w = "full", wrap = true, key = "kanban-imported",
      text{ im.key or "", size = "xs", mono = true, color = "text-muted" },
      text{ im.line or "", size = "xs", color = "text-muted", wrap = true, grow = 1 },
      button{ im.open_label or "Open", icon = "link", variant = "ghost", dense = true, on = "issue", arg = "", key = "kanban-issue", disabled = not im.open, help = im.open_help } }
  end
  if t.plan then
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ "In Backlog until you move it: a lead takes tickets from Planned.", size = "xs", color = "text-muted", wrap = true, grow = 1 },
      button{ "Move to Planned", tone = "blue", dense = true, on = "plan", arg = "", key = "kanban-plan" } }
  end
  if t.loading then kids[#kids + 1] = note("Reading the ticket…") end
  if nz(t.description) then kids[#kids + 1] = text{ t.description, wrap = true, pills = t.sandbox or true, key = "kanban-panel-description" } end
  if #(t.files or {}) > 0 then
    local pics = { gap = 8, wrap = true }
    local chips = { gap = 8, wrap = true, w = "full" }
    for _, f in ipairs(t.files) do
      if f.img then pics[#pics + 1] = image{ key = f.img, max_w = 180, max_h = 120 } end
      chips[#chips + 1] = row{ gap = 4, px = 8, h = 24, radius = "pill", fill = "surface-alt", border = "elevated", key = f.el, on = "file", arg = f.id, help = f.help,
        icon{ f.icon or "file", size = 12, color = "text-muted" },
        text{ f.name or "", size = "xs", truncate = true, max_w = 220 },
        text{ f.size or "", size = "xs", color = "text-muted" },
        t.editable and button{ icon = "close", variant = "ghost", compact = true, on = "remove_file", arg = f.id, key = f.el_remove, help = "Remove this file", tab_stop = false } or false }
    end
    kids[#kids + 1] = column{ gap = 8, w = "full", text{ t.files_label or "Files", size = "xs", color = "text-muted" }, #pics > 0 and row(pics) or false, row(chips),
      t.files_note and text{ t.files_note, size = "xs", color = "text-muted", wrap = true, key = "kanban-files-note" } or false }
  end
  if t.priorities or t.movable or t.validate then
    local r = { gap = 8, w = "full", wrap = true }
    if t.priorities then r[#r + 1] = select{ id = "kanban-priority", value = t.priority or "none", items = t.priorities, on = "priority" } end
    if t.movable then
      r[#r + 1] = button{ "Move up", variant = "ghost", dense = true, on = "move", arg = "up", key = "kanban-move-up", help = "Earlier in Planned, among its priority" }
      r[#r + 1] = button{ "Move down", variant = "ghost", dense = true, on = "move", arg = "down", key = "kanban-move-down", help = "Later in Planned, among its priority" }
      r[#r + 1] = button{ "To the top", variant = "ghost", dense = true, on = "move", arg = "top", key = "kanban-move-top" }
    end
    if t.validate then r[#r + 1] = checkbox{ "Validated on my machine", value = t.validate.on == true, on = "validate", key = "kanban-validate" } end
    kids[#kids + 1] = row(r)
  elseif nz(t.move_why) and not t.thread_card then
    kids[#kids + 1] = text{ t.move_why, size = "xs", color = "text-muted", wrap = true }
  end
  if t.thread then
    local th = t.thread
    kids[#kids + 1] = row{ gap = 8, w = "full", wrap = true, key = "kanban-thread",
      dot{ tone = th.tone or "neutral" },
      text{ th.name or "", size = "sm", mono = true, weight = "semibold", color = th.color or "text-bright" },
      text{ th.state or "", size = "xs", color = "text-muted" },
      spacer{},
      button{ "Terminal", icon = "terminal", variant = "ghost", dense = true, on = "link", arg = "terminal", key = "kanban-link-terminal" },
      button{ "Sessions", icon = "inbox", variant = "ghost", dense = true, on = "link", arg = "monitor", key = "kanban-link-monitor" },
      button{ "Commits", icon = "history", variant = "ghost", dense = true, on = "link", arg = "commits", key = "kanban-link-commits" },
      button{ "Worktrees", icon = "branch", variant = "ghost", dense = true, on = "link", arg = "worktrees", key = "kanban-link-worktrees" },
      button{ "Plan", icon = "threads", variant = "ghost", dense = true, on = "link", arg = "plan", key = "kanban-link-plan" } }
  end
  if #(t.history or {}) > 0 then
    local h = { gap = 2, w = "full", key = "kanban-history", text{ "History", size = "xs", color = "text-muted" } }
    for _, e in ipairs(t.history) do
      h[#h + 1] = row{ gap = 8, w = "full", dot{ tone = e.tone or "neutral" }, text{ e.text or "", size = "xs", grow = 1, truncate = true, help = e.text }, text{ e.when or "", size = "xs", color = "text-muted" } }
    end
    kids[#kids + 1] = column(h)
  end
  if not t.thread_card then
    kids[#kids + 1] = row{ gap = 8, w = "full",
      spacer{},
      t.send_why and text{ t.send_why, size = "xs", color = "text-muted", key = "kanban-send-why" } or false,
      button{ "Send to the lead", icon = "send", tone = "blue", on = "send", key = "kanban-send", disabled = t.send_why ~= nil,
        help = t.send_why or "Types one fixed line into the lead's session: the ticket's number and how to read it" } }
  end
  return column{ pin = { top = 0, right = 0, bottom = 0 }, w = w, pad = 16, fill = "surface", border = "elevated", radius = "lg", key = "kanban-panel",
    scroll{ key = "kanban-panel-scroll", column(kids) } }
end

return {
  render = function(_, data)
    local ui = data.kanban_ui or {}
    local ov = (data.app or {}).overlay or {}
    local ow = ov.w or 1200
    local cols = ui.columns or {}
    local n = math.max(1, #cols)
    -- 180 pt a column at least, else one column at a time (900 pt for five).
    local narrow = ow < 180 * math.max(5, n)
    local gap = 8
    local col_w = math.floor((ow - 24 - gap * (n - 1)) / n)
    local body
    if ui.loading then
      body = column{ grow = 1, note("Reading the tickets…") }
    elseif narrow then
      local cur
      local items = {}
      for _, c in ipairs(cols) do
        items[#items + 1] = { id = c.id, label = c.picker_label }
        if c.id == ui.col then cur = c end
      end
      cur = cur or cols[1]
      body = column{ gap = 8, grow = 1, w = "full",
        select{ id = "kanban-colpick", value = cur and cur.id or "", items = items, on = "col" },
        cur and lane(cur, ow - 24, ui, true) or false }
    else
      local r = { gap = gap, grow = 1, w = "full", align = "stretch", key = "kanban-board" }
      for _, c in ipairs(cols) do r[#r + 1] = lane(c, col_w, ui, false) end
      body = row(r)
    end
    local dismiss = ui.ticket and column{ pin = { left = 0, top = 0, right = 0, bottom = 0 }, key = "kanban-panel-dismiss", on = "close", opacity = 0.01, help = "Close the ticket" } or false
    return column{ h = "full", gap = 8, w = "full", px = 4,
      scope(ui),
      row{ gap = 8, w = "full", key = "kanban-summary",
        select{ id = "kanban-view", value = "kanban", items = VIEWS, on = "view", switch = true,
          help = "The Dashboard's two views: the team's conversation, or the Kanban of its tickets" },
        text{ ui.counts_line or "", size = "xs", color = "text-muted", truncate = true, grow = 1 },
        button{ "New ticket", icon = "plus", variant = "primary", dense = true, on = "new", key = "kanban-new", keys = { "N" }, disabled = ui.new_why ~= nil, help = ui.new_why or "A ticket for the sandbox shown: a title, a description, files" } },
      source(ui.source),
      ui.error and note(ui.error, "red") or false,
      body,
      dismiss,
      panel(ui.ticket, ow) }
  end,
  actions = {
    scope = function(_, _, v) return intent("kanban_scope", { sandbox = v or "" }) end,
    view = function(_, _, v) if v == "conversation" then return intent("board_show", {}) end end,
    open = function(_, id) return intent("kanban_open", { id = id }) end,
    close = function() return intent("kanban_open", { id = "" }) end,
    new = function() return intent("kanban_new", {}) end,
    col = function(_, _, v) return intent("kanban_column", { column = v }) end,
    more = function(_, c) return intent("kanban_more", { column = c }) end,
    older = function() return intent("kanban_older", {}) end,
    edit = function() return intent("kanban_edit", {}) end,
    delete = function() return intent("kanban_delete", {}) end,
    send = function() return intent("kanban_send", {}) end,
    file = function(_, id) return intent("kanban_file", { file = id }) end,
    remove_file = function(_, id) return intent("kanban_remove_file", { file = id }) end,
    move = function(_, to) return intent("kanban_move", { to = to }) end,
    priority = function(_, _, v) return intent("kanban_priority", { priority = v }) end,
    validate = function(_, _, on) return intent("kanban_validate", { on = on == true }) end,
    link = function(_, what) return intent("kanban_link", { what = what }) end,
    issue = function(_, id) return intent("kanban_issue", { id = id or "" }) end,
    plan = function(_, id) return intent("kanban_plan", { id = id or "" }) end,
    source = function() return intent("kanban_source_open", {}) end,
    source_refresh = function() return intent("kanban_source_refresh", {}) end,
  },
}
