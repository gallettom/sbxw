-- Worktrees (an overlay over the pane area, feat/worktrees.rs): the active
-- worktrees of every sandbox, one block per sandbox in its colour, one row
-- per worktree. Read only: a row opens its terminal, its commits, its files.
-- Above a sandbox's rows, the branch graph of its unmerged worktrees (laid
-- out by worktrees_graph.rs in columns and rows; drawn here in points with
-- the `paint` prop): the base branch on top, each thread forking from its
-- merge-base to its tip. A lane's label selects its row; a dot opens that
-- commit in Commits.

-- The graph's measures (points).
local ROW, PAD, EARLY, CURVE = 26, 20, 6, 6

local function tone_color(t)
  if t == "blue" or t == "amber" or t == "green" or t == "red" or t == "purple" then return t end
  return "text-muted"
end

local function graphview(sb, g, width)
  local label_w = width >= 760 and 200 or 130
  local room = width - PAD - label_w
  local step = math.max(12, math.min(28, math.floor(room / math.max(1, g.cols - 1))))
  local function X(k) if k < 0 then return EARLY end return PAD + k * step end
  local function Y(r) return ROW / 2 + r * ROW end
  local y0 = Y(0)
  local strokes, dots, kids = {}, {}, {}
  local function hit(x, y, c, branch)
    kids[#kids + 1] = column{ w = 16, h = 16, radius = 8, pin = { left = x - 8, top = y - 8 },
      key = "worktrees-dot-" .. sb .. "-" .. (c.short or "?"), on = "commit", arg = { sandbox = sb, hash = c.sha, branch = branch }, hover = "hover-subtle",
      help = (c.short or "") .. " " .. (c.subject or "") .. " (open in Commits)" }
  end
  if g.earlier then strokes[#strokes + 1] = { points = { X(-1), y0, X(0), y0 }, width = 2, color = "text-dim" } end
  local nb = #(g.base or {})
  if nb > 1 then strokes[#strokes + 1] = { points = { X(0), y0, X(nb - 1), y0 }, width = 2, color = "text-muted" } end
  for _, l in ipairs(g.lanes or {}) do
    local y = Y(l.row)
    local pts
    if l.fork >= 0 then
      local x = X(l.fork)
      pts = { x, y0, x, y - CURVE }
      -- A quarter turn into the lane.
      for i = 1, 4 do
        local a = (i / 4) * math.pi / 2
        pts[#pts + 1] = x + CURVE - CURVE * math.cos(a)
        pts[#pts + 1] = y - CURVE + CURVE * math.sin(a)
      end
    else
      strokes[#strokes + 1] = { points = { X(-1), y, X(0), y }, width = 2, color = "text-dim" }
      pts = { X(0), y }
    end
    pts[#pts + 1] = X(l["end"]); pts[#pts + 1] = y
    strokes[#strokes + 1] = { points = pts, width = 2, color = l.color }
    for _, c in ipairs(l.commits or {}) do
      dots[#dots + 1] = { x = X(c.col), y = y, r = 4, color = l.color }
      hit(X(c.col), y, c, l.branch)
    end
    if (l.hidden or 0) > 0 then
      -- The commits not drawn, on the lane where they would be (a chip over the line).
      kids[#kids + 1] = row{ h = 16, px = 4, radius = "xs", fill = "bg", border = "elevated", pin = { left = X(l.fork + 1) - 11, top = y - 8 },
        key = "worktrees-gap-" .. (l.id or l.thread), help = l.hidden .. " older commits of " .. l.thread .. " not drawn",
        text{ "+" .. l.hidden, size = "xs", mono = true, color = "text-muted" } }
    end
    local behind = tonumber(l.behind) or 0
    kids[#kids + 1] = row{ gap = 4, h = 20, px = 4, radius = "xs", pin = { left = X(l["end"]) + 10, top = y - 10 }, max_w = label_w - 12,
      key = "worktrees-lane-" .. (l.id or l.thread), on = "select", arg = l.id, hover = "hover-subtle",
      help = l.thread .. ": " .. l.ahead .. " commits ahead of " .. (g.base_name or "the base") .. ", " .. behind .. " behind. Press to mark its row",
      dot{ tone = l.tone == "muted" and "neutral" or l.tone },
      text{ l.thread, size = "xs", weight = "medium", color = tone_color(l.tone), truncate = true },
      -- Narrow, the name keeps the room (the row under says the numbers).
      width >= 760 and text{ "↑" .. l.ahead .. (behind > 0 and (" ↓" .. behind) or ""), size = "xs", mono = true, color = "text-dim" } or false,
    }
  end
  for _, c in ipairs(g.base or {}) do
    dots[#dots + 1] = { x = X(c.col), y = y0, r = c.head and 5 or 3.5, color = c.head and "text-bright" or "text-muted" }
    hit(X(c.col), y0, c, nil)
  end
  kids[#kids + 1] = text{ g.base_name or "HEAD", size = "xs", mono = true, color = "text-muted", pin = { left = X(nb - 1) + 10, top = y0 - 9 } }
  local w = math.min(width, X(g.cols - 1) + label_w)
  kids.w = w; kids.h = g.rows * ROW; kids.key = "worktrees-graph-" .. sb .. (g.repo ~= "" and ("-" .. tostring(g.repo)) or "")
  kids.paint = { strokes = strokes, dots = dots }
  return column(kids)
end

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function counts(r)
  local kids = { gap = 8 }
  if r.has_branch then
    kids[#kids + 1] = text{ "↑" .. tostring(r.ahead or 0), size = "xs", mono = true, color = (r.ahead or 0) > 0 and "blue" or "text-dim",
      help = tostring(r.ahead or 0) .. " commits ahead of " .. (r.base or "the main checkout's branch") }
    kids[#kids + 1] = text{ "↓" .. tostring(r.behind or 0), size = "xs", mono = true, color = (r.behind or 0) > 0 and "amber" or "text-dim",
      help = tostring(r.behind or 0) .. " commits behind " .. (r.base or "the main checkout's branch") }
  else
    kids[#kids + 1] = text{ "no branch", size = "xs", color = "text-dim" }
  end
  if r.dirty ~= nil then
    kids[#kids + 1] = text{ "●" .. tostring(r.dirty), size = "xs", mono = true, color = r.dirty > 0 and "amber" or "text-dim",
      help = tostring(r.dirty) .. " uncommitted changes" }
  end
  return row(kids)
end

local function rowview(r, wide)
  local function lab(t) if wide then return t end return nil end
  return column{ gap = 4, w = "full", px = 8, py = 8, radius = "xs", key = "worktrees-row-" .. r.id, hover = "hover-subtle", hover_group = true,
    fill = r.selected and "active-bg" or nil,
    row{ gap = 8, w = "full",
      icon{ "branch", size = 12, color = r.lane_color or "blue", help = r.lane_color and "Its lane in the graph above" or nil },
      text{ r.path or "", size = "sm", mono = true, color = "text-bright", truncate = true, grow = 1, min_w = 120, help = r.full },
      text{ r.thread or "", size = "xs", color = "text-muted", truncate = true, max_w = 140, help = "The thread that owns it" },
      nz(r.word) and badge{ r.word, tone = r.tone or "neutral" } or false,
      r.merged and badge{ "merged", tone = "green", help = "The main checkout's branch holds it" } or false,
      counts(r),
    },
    row{ gap = 8, w = "full",
      spacer{ w = 20, grow = 0 },
      text{ r.branch or "", size = "xs", mono = true, color = "text-muted", truncate = true, max_w = 220 },
      nz(r.subject) and text{ r.subject, size = "xs", color = "text", truncate = true, grow = 1, min_w = 80, help = r.subject } or spacer{},
      nz(r.when) and text{ r.when, size = "xs", color = "text-dim" } or false,
      button{ lab("Terminal"), key = "worktrees-terminal-" .. r.id, icon = "terminal", on = "terminal", arg = r.id, variant = "ghost", tone = "green", dense = true,
        help = "Open its thread's terminal" },
      button{ lab("Commits"), key = "worktrees-commits-" .. r.id, icon = "history", on = "commits", arg = r.id, variant = "ghost", tone = "blue", dense = true,
        disabled = not r.has_branch, help = "Its branch in the Commits view" },
      button{ lab("Files"), key = "worktrees-files-" .. r.id, icon = "folder", on = "files", arg = r.id, variant = "ghost", tone = "blue", dense = true,
        help = "The file explorer at this worktree" },
    },
  }
end

return {
  render = function(_, data)
    local ui = data.worktrees_ui or {}
    local over = (data.app or {}).overlay or {}
    local width = (over.w or 1000) - 40
    local wide = (over.w or 1000) >= 760
    local list = { gap = 12, w = "full", key = "worktrees-list" }
    for _, b in ipairs(ui.blocks or {}) do
      local blk = { gap = 2, w = "full", radius = "md", pad = 4, key = "worktrees-block-" .. b.sandbox, border = "elevated",
        row{ gap = 8, px = 4, h = 28,
          column{ w = 8, h = 8, radius = 2, fill = b.color or "text-dim" },
          text{ b.sandbox, size = "caption", weight = "semibold", color = "text-bright" },
          text{ tostring(#(b.rows or {})), size = "caption", color = "text-muted" },
        },
      }
      for _, gr in ipairs(b.graphs or {}) do
        local g = wide and gr.wide or gr.narrow
        blk[#blk + 1] = column{ w = "full", px = 4, py = 4, clip = true,
          (#(b.graphs or {}) > 1 and g.repo ~= "") and text{ g.repo, size = "xs", mono = true, color = "text-dim" } or false,
          graphview(b.sandbox, g, width) }
      end
      for _, r in ipairs(b.rows or {}) do blk[#blk + 1] = rowview(r, wide) end
      list[#list + 1] = column(blk)
    end
    local more = more_rows(ui.count, { key = "worktrees-more", on = "more", what = "worktrees", hint = "Filter to find one" })
    if more then list[#list + 1] = more end
    if #(ui.blocks or {}) == 0 then
      list[#list + 1] = note(ui.loading and "Reading the worktrees…" or (nz(ui.filter) and "No worktree matches the filter." or "No active worktree: threads with their own worktree show here."))
    end
    if #(ui.none or {}) > 0 then
      list[#list + 1] = note("No worktree: " .. table.concat(ui.none, ", "), "text-dim")
    end
    for _, n in ipairs(ui.notes or {}) do list[#list + 1] = note(n.text, n.tone == "red" and "red" or "text-dim") end
    return column{ h = "full", gap = 8, w = "full",
      row{ gap = 8, w = "full",
        input{ id = "worktrees-filter", value = ui.filter or "", placeholder = "Filter by sandbox, thread, branch or path", on = "filter" },
        button{ icon = "reload", key = "worktrees-refresh", on = "refresh", variant = "ghost", help = "Read the worktrees again" },
      },
      scroll{ key = "worktrees-scroll", column(list) },
    }
  end,
  actions = {
    filter = function(_, _, t) return intent("worktrees_filter", { text = t or "" }) end,
    refresh = function() return intent("worktrees_refresh", {}) end,
    more = function() return intent("worktrees_more", {}) end,
    terminal = function(_, id) return intent("worktrees_terminal", { id = id }) end,
    commits = function(_, id) return intent("worktrees_commits", { id = id }) end,
    files = function(_, id) return intent("worktrees_files", { id = id }) end,
    select = function(_, id) return intent("worktrees_select", { id = id }) end,
    commit = function(_, a) return intent("commits_show", { sandbox = a.sandbox, hash = a.hash, branch = a.branch }) end,
  },
}
