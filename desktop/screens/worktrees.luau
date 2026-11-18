-- Worktrees (an overlay over the pane area, feat/worktrees.rs): the active
-- worktrees of every sandbox, one block per sandbox in its colour, one row
-- per worktree. Read only: a row opens its terminal, its commits, its files.

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

local function rowview(r)
  return column{ gap = 4, w = "full", px = 8, py = 8, radius = "xs", key = "worktrees-row-" .. r.id, hover = "hover-subtle", hover_group = true,
    row{ gap = 8, w = "full",
      icon{ "branch", size = 12, color = "blue" },
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
      button{ "Terminal", key = "worktrees-terminal-" .. r.id, icon = "terminal", on = "terminal", arg = r.id, variant = "ghost", tone = "green", dense = true,
        help = "Open its thread's terminal" },
      button{ "Commits", key = "worktrees-commits-" .. r.id, icon = "history", on = "commits", arg = r.id, variant = "ghost", tone = "blue", dense = true,
        disabled = not r.has_branch, help = "Its branch in the Commits view" },
      button{ "Files", key = "worktrees-files-" .. r.id, icon = "folder", on = "files", arg = r.id, variant = "ghost", tone = "blue", dense = true,
        help = "The file explorer at this worktree" },
    },
  }
end

return {
  render = function(_, data)
    local ui = data.worktrees_ui or {}
    local list = { gap = 12, w = "full", key = "worktrees-list" }
    for _, b in ipairs(ui.blocks or {}) do
      local blk = { gap = 2, w = "full", radius = "md", pad = 4, key = "worktrees-block-" .. b.sandbox, border = "elevated",
        row{ gap = 8, px = 4, h = 28,
          column{ w = 8, h = 8, radius = 2, fill = b.color or "text-dim" },
          text{ b.sandbox, size = "caption", weight = "semibold", color = "text-bright" },
          text{ tostring(#(b.rows or {})), size = "caption", color = "text-muted" },
        },
      }
      for _, r in ipairs(b.rows or {}) do blk[#blk + 1] = rowview(r) end
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
  },
}
