-- A ticket's form (feat/kanban/), a dialog over the window: the sandbox it
-- is for, a title, a description (a chat input: the person writes to an
-- agent), a priority, files attached by the file dialog, a drop on this
-- dialog or a pasted picture. Paths never pass through here: the core hands
-- them to the feature's Rust, which checks them (drops::guardrails) and
-- shows only names, sizes and what to look at.

local function nz(s) return s ~= nil and s ~= "" end

local PRIOS = { { id = "none", label = "No priority" }, { id = "p1", label = "P1" }, { id = "p2", label = "P2" }, { id = "p3", label = "P3" } }

return {
  render = function(_, data)
    local f = data.kanban_form or {}
    -- The description is the part that grows with the window (10 lines at
    -- 816 pt tall, 4 at 480); the rest keeps its height. The fields scroll
    -- in the dialog's body, Cancel and Save stay under it.
    local win_h = tonumber(((data.app or {}).window or {}).h) or 816
    local rows = math.max(4, math.min(10, math.floor((win_h - 440) / 20)))
    local kids = { gap = 12, w = "full", pad = 4, key = "kanban-form-fields" }
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ f.heading or "New ticket", size = "lg", weight = "semibold", color = "text-bright" },
      spacer{},
      nz(f.edit) and text{ f.sandbox or "", size = "sm", mono = true, color = "text-muted" } or
        select{ id = "kanban-form-sandbox", value = f.sandbox or "", on = "sandbox", scope = { kind = "sandbox" }, placeholder = "Pick a sandbox" } }
    kids[#kids + 1] = column{ gap = 4, w = "full",
      row{ gap = 8, w = "full", text{ "Title", size = "sm", color = "text-muted" }, spacer{},
        nz(f.title_count) and text{ f.title_count, size = "xs", color = f.title_over and "red" or "text-dim" } or false },
      input{ id = "kanban-form-title", value = f.title_text or "", placeholder = "What should be done, in one line", on = "title", on_submit = "save" } }
    kids[#kids + 1] = column{ gap = 4, w = "full",
      row{ gap = 8, w = "full", text{ "Description", size = "sm", color = "text-muted" }, spacer{},
        nz(f.description_count) and text{ f.description_count, size = "xs", color = f.description_over and "red" or "text-dim" } or false },
      textarea{ id = "kanban-form-description", value = f.description or "", rows = rows, tone = "compose", placeholder = "What it is for, what done looks like, anything the lead should know. Plain text.", on = "description", on_submit = "save" } }
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ "Priority", size = "sm", color = "text-muted" },
      select{ id = "kanban-form-priority", value = f.priority or "none", items = PRIOS, on = "priority", w = 200 } }
    local files = { gap = 8, w = "full", key = "kanban-form-files",
      row{ gap = 8, w = "full",
        text{ "Files", size = "sm", color = "text-muted" },
        nz(f.files_label) and text{ f.files_label, size = "xs", color = "text-dim" } or false,
        spacer{},
        button{ "Attach…", icon = "file", variant = "ghost", dense = true, on = "attach", key = "kanban-form-attach", disabled = f.attach_why ~= nil, help = f.attach_why or "Pick files: pictures, PDFs, text (20 MB each, 20 files, 100 MB in all; archives are refused)" },
        button{ "Paste picture", icon = "image", variant = "ghost", dense = true, on = "paste", key = "kanban-form-paste", disabled = f.attach_why ~= nil, help = f.attach_why or "Attach the picture on the clipboard" } } }
    for _, x in ipairs(f.files or {}) do
      files[#files + 1] = row{ gap = 8, w = "full", key = x.el, px = 8, py = 4, radius = "sm", fill = x.look and "amber-tint" or "surface-alt",
        icon{ "file", size = 12, color = "text-muted" },
        text{ x.name or "", size = "xs", truncate = true, grow = 1, help = x.name },
        text{ x.size or "", size = "xs", color = "text-dim" },
        x.look and icon{ "warning", size = 12, color = "amber" } or false,
        button{ icon = "close", variant = "ghost", compact = true, on = "unattach", arg = x.i, key = x.el .. "-remove", help = "Take this file back" } }
      if x.look then files[#files + 1] = text{ "Look first: " .. x.look, size = "xs", color = "amber", wrap = true } end
    end
    -- A drop zone to aim at (a drop anywhere on the dialog also works).
    files[#files + 1] = column{ w = "full", h = 72, radius = "sm", fill = "surface-alt", border = "border", key = "kanban-form-drop", align = "center", justify = "center",
      text{ #(f.files or {}) == 0 and "Drop files here, or attach them." or "Drop more files here.", size = "xs", color = "text-dim" } }
    kids[#kids + 1] = column(files)
    local foot = { gap = 8, w = "full", key = "kanban-form-foot" }
    if nz(f.error) then foot[#foot + 1] = text{ f.error, size = "sm", color = "red", wrap = true, key = "kanban-form-error" } end
    foot[#foot + 1] = row{ gap = 8, w = "full",
      spacer{},
      button{ "Cancel", on = "cancel", key = "kanban-form-cancel" },
      button{ f.saving and "Saving…" or "Save", variant = "primary", on = "save", key = "kanban-form-save", disabled = f.save_why ~= nil, help = f.save_why or "Save the ticket on this computer, with its files", keys = { "⌘", "Return" } } }
    return column{ gap = 12, w = "full", h = "full", key = "kanban-form",
      scroll{ grow = 1, key = "kanban-form-scroll", column(kids) },
      column(foot) }
  end,
  actions = {
    title = function(_, _, t) return intent("kanban_form", { title = t or "" }) end,
    description = function(_, _, t) return intent("kanban_form", { description = t or "" }) end,
    priority = function(_, _, v) return intent("kanban_form", { priority = v }) end,
    sandbox = function(_, _, v) return intent("kanban_form", { sandbox = v or "" }) end,
    attach = function() return intent("kanban_attach", {}) end,
    paste = function() return intent("kanban_paste", {}) end,
    unattach = function(_, i) return intent("kanban_unattach", { index = i }) end,
    save = function() return intent("kanban_save", {}) end,
    cancel = function() return intent("kanban_cancel", {}) end,
  },
}
