-- The Project panel (docked on the right): a sandbox's deliverables
-- (.sbxw-artifacts), its files, what was written lately, and uploads into
-- it (assets/js/files.js, upload.js).
--
-- Everything listed and previewed here comes from inside the sandbox: names
-- and text are drawn as text or through `markdown`, pictures by the core.
-- Downloads, opening a file with this computer's app, deleting and uploading
-- are intents the core runs or confirms in its own dialog; this screen never
-- sees a path of this computer, nor any file's bytes.

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function head(t)
  return text{ string.upper(t), size = "xxs", weight = "semibold", color = "text-muted" }
end

local function tabs(ui)
  local function tab(id, label, count)
    local on = ui.tab == id
    return button{ label, key = "files-tab-" .. id, on = "tab", arg = id, variant = on and "neutral" or "ghost",
      color = on and "text-bright" or "text-muted", help = count and (count .. " written in the last hour") or nil }
  end
  local fresh = (ui.recent_fresh or 0) > 0 and ui.recent_fresh or nil
  local map_label = "Code map"
  if ui.run and ui.run.phase == "writing" then
    map_label = ui.run.label
  elseif not ui.has_map then
    map_label = "Generate code map"
  end
  return row{ gap = 4, w = "full",
    tab("artifacts", "Artifacts"),
    tab("files", "Files"),
    tab("recent", fresh and ("Recent · " .. fresh) or "Recent", fresh),
    spacer{},
    button{ map_label, key = "files-codemap", icon = "sparkle", on = "codemap", arg = { sandbox = ui.sandbox, has_map = ui.has_map == true }, variant = "ghost",
      disabled = ui.run and ui.run.phase == "writing" and not ui.has_map,
      help = ui.has_map and "Read the code map" or "Ask this sandbox's agent to write one, in a background session" },
    button{ icon = "reload", key = "files-refresh", on = "refresh", variant = "ghost", help = "Read the lists again" },
  }
end

local function uploads(ui)
  local kids = { gap = 6, w = "full", px = 8, py = 5, radius = "sm", border = "border", fill = "surface-alt", key = "files-drop" }
  local staged = ui.staged or {}
  kids[#kids + 1] = row{ gap = 6, w = "full",
    icon{ "plus", size = 12, color = "text-muted" },
    text{ #staged == 0 and "Upload: drop files here or on a terminal" or ("Upload into " .. (ui.sandbox or "?")), size = "sm", truncate = true, grow = 1,
      help = "Or paste a picture. Up to " .. (ui.upload_max or "128 MB") .. " each; the path is typed into the agent's prompt, not submitted." },
    button{ "Choose…", key = "files-upload-pick", on = "pick", variant = "ghost", help = "The system's file dialog" },
    button{ "Paste", key = "files-upload-paste", on = "paste", variant = "ghost", help = "The picture on the clipboard" },
  }
  if #staged == 0 then
    -- nothing more: the line above says it
  else
    for _, s in ipairs(staged) do
      local sending = s.state == "sending"
      kids[#kids + 1] = row{ gap = 6, w = "full", key = "files-staged-" .. s.index,
        icon{ "file", size = 11, color = "text-muted" },
        column{ gap = 2, grow = 1,
          text{ s.name, size = "sm", truncate = true },
          nz(s.problem) and text{ s.problem, size = "xs", color = "red", wrap = true }
            or text{ s.size .. (s.to and ("  →  " .. s.to) or ""), size = "xs", color = "text-muted", truncate = true },
          sending and progress{ value = s.progress or 0 } or false,
        },
        button{ sending and "Sending…" or "Upload", key = "files-upload-send-" .. s.index, icon = "send", variant = s.index == 1 and "primary" or "neutral",
          on = "send", arg = s.index, disabled = nz(s.problem) or sending, help = "The app shows the file, its size and where it goes first" },
        button{ icon = "close", key = "files-unstage-" .. s.index, on = "unstage", arg = s.index, variant = "ghost", help = "Leave it out", disabled = sending },
      }
    end
    if #staged > 1 then
      kids[#kids + 1] = row{ w = "full", spacer{}, button{ "Clear all", key = "files-unstage-all", on = "unstage", variant = "ghost" } }
    end
  end
  if ui.saved then
    kids[#kids + 1] = row{ gap = 6, w = "full", key = "files-saved",
      icon{ "check", size = 11, color = "green" },
      text{ "Saved " .. ui.saved.name, size = "xs", color = "text-muted", truncate = true, grow = 1, help = ui.saved.path },
      button{ "Show in folder", key = "files-show-saved", on = "show_saved", variant = "ghost" },
    }
  end
  return column(kids)
end

local function filebar(ui)
  local items = {}
  for _, p in ipairs(ui.places or {}) do
    items[#items + 1] = { id = p.id, label = p.label, icon = p.icon }
  end
  return column{ gap = 6, w = "full",
    row{ gap = 4, w = "full",
      button{ "←", key = "files-back", on = "back", variant = "ghost", disabled = not ui.can_back, help = "Back" },
      button{ "↑", key = "files-up", on = "up", variant = "ghost", disabled = not ui.can_up, help = "The folder above" },
      input{ id = "files-path", value = ui.path or "", placeholder = "/path or ~", on_submit = "go", grow = 1 },
      ui.path and button{ icon = "flag", key = "files-star", on = "star", arg = ui.path, variant = "ghost",
        color = ui.starred and "amber" or "text-muted", help = ui.starred and "Unpin this folder" or "Pin this folder among the places" } or false,
      #items > 0 and menu{ "Places", id = "files-places", icon = "folder", items = items, on = "place" } or false,
    },
    row{ gap = 6, w = "full",
      checkbox{ "Hidden files", value = ui.hidden == true, on = "hidden", key = "files-hidden" },
      ui.offline and badge{ "stopped: workspace only", tone = "amber", help = "Read from this computer; the sandbox is not started to list a folder" } or false,
    },
  }
end

local function rowview(ui, r, i)
  local meta = {}
  if nz(r.dir) then meta[#meta + 1] = r.dir end
  if nz(r.when) then meta[#meta + 1] = r.when end
  local kids = { gap = 6, w = "full", px = 6, py = 5, radius = "xs", key = "files-row-" .. r.id, on = "select", arg = r.id,
    fill = r.selected and "active-bg" or nil,
    icon{ r.icon or "file", size = 12, color = r.kind == "dir" and "blue" or "text-muted" },
    column{ gap = 1, grow = 1,
      text{ r.name .. (r.link and " ↪" or ""), size = "sm", truncate = true, color = r.selected and "text-bright" or "text" },
      #meta > 0 and text{ table.concat(meta, " · "), size = "xs", color = "text-dim", truncate = true } or false,
    },
    r.fresh and dot{ tone = "blue", help = "Written in the last hour" } or false,
    nz(r.size) and text{ r.size, size = "xs", color = "text-muted" } or false,
  }
  if ui.tab == "recent" then
    kids[#kids + 1] = button{ icon = "folder", key = "files-show-" .. r.id, on = "show", arg = r.id, variant = "ghost", help = "Show in folder" }
  end
  return row(kids)
end

local function rows(ui)
  local list = { gap = 1 }
  local last
  for i, r in ipairs(ui.rows or {}) do
    if ui.tab == "recent" and r.group ~= last then
      last = r.group
      list[#list + 1] = row{ py = 4, head(r.group or "") }
    end
    list[#list + 1] = rowview(ui, r, i)
  end
  local err = ui.errors and ui.errors[ui.tab]
  if nz(err) then
    list[#list + 1] = note(err, "red")
  elseif #(ui.rows or {}) == 0 then
    local loading = ui.loading and ui.loading[ui.tab]
    local empty = {
      artifacts = "No deliverables yet. Agents put documents, pictures and reports in .sbxw-artifacts/; they show here.",
      files = "Nothing here" .. (nz(ui.filter) and " matches the filter." or "."),
      recent = "Nothing written in the last " .. (ui.recent_days or 7) .. " days.",
    }
    list[#list + 1] = note(loading and "Loading…" or empty[ui.tab] or "")
  end
  return scroll(list)
end

local function preview(ui)
  local s = ui.selected
  if not s then
    return column{ h = "full", pad = 8, note("Pick a file to preview it.") }
  end
  local p = ui.preview or {}
  local isfile = s.kind == "file"
  local acts = { gap = 4, w = "full",
    isfile and button{ "Download", key = "files-download", icon = "chevron-down", on = "download", help = "Into Downloads (never over a file there)" } or false,
    isfile and button{ icon = "folder", key = "files-reveal", on = "reveal", variant = "ghost", help = "Download it and show it in its folder" } or false,
    isfile and button{ "Open", key = "files-open-with", on = "open_with", variant = "ghost", help = "With this computer's app; the app asks first, and never opens programs or scripts" } or false,
    isfile and button{ "Save as…", key = "files-save-as", on = "save_as", variant = "ghost", help = "Choose where (the app asks first)" } or false,
    button{ icon = "copy", key = "files-copy-path", on = "copy", variant = "ghost", help = "Copy the path, as the agent sees it" },
    spacer{},
    s.can_delete and button{ icon = "trash", key = "files-delete", on = "delete", variant = "ghost", color = "red", help = "Delete it (the app asks first)" } or false,
  }
  local body
  if p.state == "loading" then
    body = note("Loading…")
  elseif p.state == "markdown" then
    body = scroll{ markdown{ p.text or "", on = "link", size = "sm" }, nz(p.note) and note(p.note) or false }
  elseif p.state == "text" or p.state == "html" then
    body = scroll{
      p.state == "html" and note("An HTML page, shown as its source: the app does not draw pages. Open it to see it in your browser.", "amber") or false,
      text{ p.text or "", mono = true, size = "xs", wrap = true }, nz(p.note) and note(p.note) or false }
  elseif p.state == "image" then
    body = column{ gap = 4, image{ key = p.key, max_w = 420, max_h = 320 }, note((p.w or "?") .. " × " .. (p.h or "?") .. " px") }
  elseif p.state == "error" then
    body = note(p.text or "Could not read it.", "red")
  else
    body = note(p.text or "")
  end
  return column{ gap = 6, h = "full", pad = 8,
    text{ s.name, weight = "semibold", truncate = true, color = "text-bright" },
    text{ s.type .. (isfile and (" · " .. s.size .. " · " .. s.modified) or ""), size = "xs", color = "text-muted", truncate = true },
    text{ s.path or "", size = "xs", mono = true, color = "text-dim", truncate = true, help = s.path },
    row(acts),
    divider{},
    body,
  }
end

return {
  render = function(_, data)
    local ui = data.files_ui
    if not ui or not ui.sandbox then
      return column{ pad = 12, note("Open a sandbox's Project from its pane bar.") }
    end
    return column{ h = "full", gap = 8, pad = 10,
      row{ gap = 6, w = "full", badge{ ui.sandbox, tone = "blue" },
        nz(ui.artifacts_dir) and text{ ui.artifacts_dir, size = "xs", color = "text-dim", mono = true, truncate = true, grow = 1 } or spacer{} },
      tabs(ui),
      uploads(ui),
      ui.tab == "files" and filebar(ui) or false,
      input{ id = "files-filter", value = ui.filter or "", placeholder = "Filter by name", on = "filter" },
      split{ id = "files-split", ratio = 0.5, rows(ui), preview(ui) },
    }
  end,
  actions = {
    tab = function(_, t) return intent("files_tab", { tab = t }) end,
    refresh = function() return intent("files_refresh", {}) end,
    codemap = function(_, a)
      if a.has_map then return intent("codemap_open", { sandbox = a.sandbox }) end
      return intent("codemap_generate", { sandbox = a.sandbox })
    end,
    pick = function() return intent("files_upload_pick", {}) end,
    paste = function() return intent("files_upload_paste", {}) end,
    unstage = function(_, i) if i then return intent("files_upload_remove", { index = i }) end return intent("files_upload_remove", {}) end,
    send = function(_, i) return intent("files_upload_send", { index = i }) end,
    show_saved = function() return intent("files_show_saved", {}) end,
    back = function() return intent("files_back", {}) end,
    up = function() return intent("files_up", {}) end,
    go = function(_, _, path) if path and path ~= "" then return intent("files_browse", { path = path }) end end,
    star = function(_, p) return intent("files_star", { path = p }) end,
    place = function(_, _, id) return intent("files_place", { id = id }) end,
    hidden = function(_, _, on) return intent("files_hidden", { on = on }) end,
    filter = function(_, _, t) return intent("files_filter", { text = t or "" }) end,
    select = function(_, id) return intent("files_select", { id = id }) end,
    show = function(_, id) return intent("files_show_in_folder", { id = id }) end,
    download = function() return intent("files_download", {}) end,
    reveal = function() return intent("files_reveal", {}) end,
    open_with = function() return intent("files_open_with", {}) end,
    save_as = function() return intent("files_save_as", {}) end,
    copy = function() return intent("files_copy_path", {}) end,
    delete = function() return intent("files_delete", {}) end,
    link = function(_, _, url) return intent("open_link", { url = url }) end,
  },
}
