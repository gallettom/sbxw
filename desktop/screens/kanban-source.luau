-- A sandbox's ticket source (feat/kanban/), a dialog over the window: the
-- tracker (GitHub only for now, shown so the choice is visible), the
-- repository (filled from the sandbox's own when the daemon detected it),
-- the filter label, whether a merged ticket closes its issue, and one line
-- saying what sbxw writes to the tracker. Save and Disconnect each go
-- through the core's confirmation (an Approval): what sbxw writes outward is
-- the person's decision. The fields scroll in the dialog's body; Cancel and
-- Save stay under it.

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "xs", color = color or "text-muted", wrap = true }
end

return {
  render = function(_, data)
    local f = data.kanban_source_form or {}
    local kids = { gap = 16, w = "full", pad = 4, key = "kanban-source-fields" }
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ f.heading or "Ticket source", size = "lg", weight = "semibold", color = "text-bright" },
      spacer{},
      text{ f.sandbox or "", size = "sm", mono = true, color = "text-muted" } }
    kids[#kids + 1] = note("Issues of a repository come in as tickets in Backlog. You move one to Planned when it should be worked on.")
    kids[#kids + 1] = column{ gap = 4, w = "full",
      text{ "Tracker", size = "sm", color = "text-muted" },
      select{ id = "kanban-source-tracker", value = f.tracker or "github", items = f.trackers or {}, on = "tracker", w = 200 },
      nz(f.tracker_note) and note(f.tracker_note) or false }
    kids[#kids + 1] = column{ gap = 4, w = "full",
      text{ "Repository", size = "sm", color = "text-muted" },
      input{ id = "kanban-source-repo", value = f.repo or "", placeholder = "owner/name", on = "repo", on_submit = "save" },
      nz(f.repo_note) and note(f.repo_note) or false }
    kids[#kids + 1] = column{ gap = 4, w = "full",
      text{ "Filter label", size = "sm", color = "text-muted" },
      input{ id = "kanban-source-label", value = f.label or "", placeholder = "sbxw", on = "label", on_submit = "save" },
      nz(f.label_note) and note(f.label_note) or false }
    kids[#kids + 1] = checkbox{ "Close the issue when its ticket is merged", value = f.auto_close == true, on = "auto_close", key = "kanban-source-close" }
    kids[#kids + 1] = row{ gap = 8, w = "full", px = 8, py = 8, radius = "sm", fill = "surface-alt", key = "kanban-source-writes",
      icon{ "info", size = 12, color = "text-muted" },
      text{ f.writes or "", size = "xs", color = "text-muted", wrap = true, grow = 1 } }
    local foot = { gap = 8, w = "full", key = "kanban-source-foot" }
    if nz(f.error) then foot[#foot + 1] = text{ f.error, size = "sm", color = "red", wrap = true, key = "kanban-source-error" } end
    foot[#foot + 1] = row{ gap = 8, w = "full",
      f.configured and button{ "Disconnect", variant = "ghost", tone = "red", on = "remove", key = "kanban-source-remove", disabled = f.saving == true, help = "Stop importing; the imported tickets still in Backlog are removed (asked first)" } or false,
      spacer{},
      button{ "Cancel", on = "cancel", key = "kanban-source-cancel" },
      button{ f.saving and "Saving…" or (f.save_label or "Save"), variant = "primary", on = "save", key = "kanban-source-save", busy = f.saving and true or false, disabled = not f.saving and f.save_why ~= nil, help = f.save_why or "Asked first: what sbxw writes to the tracker" } }
    return column{ gap = 12, w = "full", h = "full", key = "kanban-source",
      scroll{ grow = 1, key = "kanban-source-scroll", column(kids) },
      column(foot) }
  end,
  actions = {
    tracker = function(_, _, v) return intent("kanban_source_field", { tracker = v }) end,
    repo = function(_, _, t) return intent("kanban_source_field", { repo = t or "" }) end,
    label = function(_, _, t) return intent("kanban_source_field", { label = t or "" }) end,
    auto_close = function(_, _, on) return intent("kanban_source_field", { auto_close = on == true }) end,
    save = function() return intent("kanban_source_save", {}) end,
    remove = function() return intent("kanban_source_remove", {}) end,
    cancel = function() return intent("kanban_source_cancel", {}) end,
  },
}
