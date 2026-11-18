-- The file explorer, an overlay over the pane area (it replaced the docked
-- Project panel). Across the top: where you are (the folder as pressable
-- segments, back, up, copy, Go to…), Upload and the "…" menu. Left: the
-- sandbox picker, the Locations (this sandbox: Home, Workspace, Temporary
-- files, the root; shared with sbxw: Artifacts, Recent; its worktrees;
-- pinned folders), then the list (the filter with Hidden and the sort, the
-- rows as a tree whose folders open in place); right, the reader (Markdown at a reading measure with its outline,
-- text and code with line numbers, pictures). A sandbox's deliverables
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
  return text{ t, size = "caption", weight = "semibold", color = "text-muted" }
end

-- The code map's button: "Code map" when the sandbox has one, else
-- "Generate code map" (or the run's progress). Beside the Workspace location.
local function codemap_button(ui)
  local label = "Code map"
  if ui.run and ui.run.phase == "writing" then
    label = ui.run.label
  elseif not ui.has_map then
    label = "Generate code map"
  end
  return button{ label, key = "files-codemap", icon = "sparkle", on = "codemap", arg = { sandbox = ui.sandbox, has_map = ui.has_map == true }, variant = "ghost", dense = true,
    disabled = ui.run and ui.run.phase == "writing" and not ui.has_map,
    help = ui.has_map and "Read the code map" or "Ask this sandbox's agent to write one, in a background session" }
end

-- WHERE TO LOOK: the Locations, a visible list grouped by where the files
-- are (this sandbox, shared with sbxw, its worktrees, what you pinned), the
-- current one marked. Narrow, a menu with the current one as its label.
local function locations(ui, narrow)
  local groups = ui.locations or {}
  if narrow then
    local items, label = {}, "Locations"
    for _, g in ipairs(groups) do
      for _, x in ipairs(g.items or {}) do
        items[#items + 1] = { id = x.id, label = x.label .. "  (" .. g.group .. ")", icon = x.icon }
        if x.id == ui.location then label = x.label end
      end
    end
    return row{ gap = 8, w = "full",
      #items > 0 and menu{ label, id = "files-locations", icon = "folder", items = items, on = "place" } or false,
      spacer{},
      codemap_button(ui),
    }
  end
  local kids = { gap = 2, w = "full", key = "files-locations-list",
    text{ "Locations", size = "caption", weight = "semibold", color = "text-muted" },
    text{ "Inside " .. (ui.sandbox or "the sandbox") .. ", as its agent sees them", size = "xs", color = "text-dim", truncate = true,
      help = "These are the sandbox's files, not this computer's: only the workspace (and any mount) is a folder of this computer, shared at the same path" },
  }
  for gi, g in ipairs(groups) do
    -- A group reads as a group: more air above its label than between entries, a quieter label.
    kids[#kids + 1] = column{ w = "full", py = 4, key = "files-group-" .. g.group,
      spacer{ h = gi == 1 and 0 or 4, grow = 0 },
      text{ g.group, size = "xs", color = "text-dim" } }
    for _, x in ipairs(g.items or {}) do
      local here = x.id == ui.location
      local item = row{ gap = 8, grow = 1, min_w = 0, px = 8, h = 22, radius = "xs",
        key = x.thread and ("files-worktree-" .. x.thread) or ("files-loc-" .. x.id),
        on = x.thread and "worktree" or "place", arg = x.thread and { sandbox = ui.sandbox, thread = x.thread } or x.id,
        fill = here and "active-bg" or nil, hover = "hover-subtle", help = x.help, selected = here,
        icon{ x.icon or "folder", size = 12, color = x.thread and "blue" or (here and "text-bright" or "text-muted") },
        text{ x.label, size = "sm", truncate = true, grow = 1, color = here and "text-bright" or "text" },
      }
      kids[#kids + 1] = x.workspace and row{ gap = 4, w = "full", item, codemap_button(ui) } or item
    end
  end
  return column(kids)
end

-- WHERE AM I: the folder as pressable segments (the home as ~), long paths
-- folding in the middle (the hidden segments in a menu), with back, up,
-- copy and "Go to…" (a typed path).
local function crumbs(state, ui, narrow)
  local cs = ui.crumbs or {}
  local kids = { gap = 4, grow = 1, min_w = 0, key = "files-crumbs",
    button{ "←", key = "files-back", on = "back", variant = "ghost", dense = true, disabled = not ui.can_back, help = "Back to the folder before" },
    button{ "↑", key = "files-up", on = "up", variant = "ghost", dense = true, disabled = not (ui.tab == "files" and ui.can_up), help = "The folder above" },
  }
  if state.goto then
    kids[#kids + 1] = input{ id = "files-path", value = ui.path or "", placeholder = "/path or ~/path, then Return", on_submit = "go", grow = 1 }
    kids[#kids + 1] = button{ icon = "close", key = "files-goto-close", on = "goto", variant = "ghost", dense = true, help = "Back to the path" }
    return row(kids)
  end
  local max = narrow and 3 or 7
  local function seg(i, c, last)
    return button{ c.label, key = "files-crumb-" .. i, on = "crumb", arg = c.path, variant = last and "neutral" or "ghost", dense = true,
      color = last and "text-bright" or "text-muted", disabled = c.path == nil or last, help = c.path }
  end
  local n = #cs
  local path = row{ gap = 0, grow = 1, min_w = 0, clip = true }
  local function add(x) path[#path + 1] = x end
  for i, c in ipairs(cs) do
    local fold_from, fold_to = 2, n - (max - 2)
    if n > max and i >= fold_from and i <= fold_to then
      if i == fold_from then
        local items = {}
        for j = fold_from, fold_to do items[#items + 1] = { id = cs[j].path, label = cs[j].label } end
        add(menu{ "…", id = "files-crumbs-more", items = items, on = "crumb_pick" })
        add(text{ "/", size = "sm", color = "text-dim" })
      end
    else
      add(seg(i, c, i == n))
      if i < n and not (i == 1 and c.label == "/") then add(text{ "/", size = "sm", color = "text-dim" }) end
    end
  end
  kids[#kids + 1] = path
  if ui.tab == "files" and ui.path then
    kids[#kids + 1] = button{ icon = "copy", key = "files-copy-folder", on = "copy_folder", arg = ui.path, variant = "ghost", dense = true, help = "Copy " .. ui.path }
    kids[#kids + 1] = button{ icon = "flag", key = "files-star", on = "star", arg = ui.path, variant = "ghost", dense = true,
      color = ui.starred and "amber" or "text-muted", help = ui.starred and "Unpin this folder" or "Pin this folder among the Locations" }
  end
  kids[#kids + 1] = button{ "Go to…", key = "files-goto", on = "goto", variant = "ghost", dense = true, help = "Type a path (~ is the agent's home)" }
  return row(kids)
end

-- Upload: one header button; this card under the header while it is open,
-- while files wait, or while files are dragged over the explorer.
local function uploads(ui, dragging)
  local kids = { gap = 8, w = "full", px = 8, py = 4, radius = "sm", border = dragging and "blue" or "border", fill = "surface-alt", key = "files-drop" }
  local staged = ui.staged or {}
  kids[#kids + 1] = row{ gap = 8, w = "full",
    icon{ "plus", size = 12, color = "text-muted" },
    text{ dragging and ("Drop to upload into " .. (ui.sandbox or "?")) or (#staged == 0 and "Upload: drop files here or on a terminal" or ("Upload into " .. (ui.sandbox or "?"))), size = "sm", truncate = true, grow = 1,
      help = "Or paste a picture. Up to " .. (ui.upload_max or "128 MB") .. " each; the path is typed into the agent's prompt, not submitted." },
    button{ "Choose…", key = "files-upload-pick", on = "pick", variant = "ghost", help = "The system's file dialog" },
    button{ "Paste", key = "files-upload-paste", on = "paste", variant = "ghost", help = "The picture on the clipboard" },
  }
  if #staged == 0 then
    -- nothing more: the line above says it
  else
    for _, s in ipairs(staged) do
      local sending = s.state == "sending"
      kids[#kids + 1] = row{ gap = 8, w = "full", key = "files-staged-" .. s.index,
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
    kids[#kids + 1] = row{ gap = 8, w = "full", key = "files-saved",
      icon{ "check", size = 11, color = "green" },
      text{ "Saved " .. ui.saved.name, size = "xs", color = "text-muted", truncate = true, grow = 1, help = ui.saved.path },
      button{ "Show in folder", key = "files-show-saved", on = "show_saved", variant = "ghost" },
    }
  end
  return column(kids)
end

-- The list's own bar: the filter (typing anywhere in the list filters too),
-- Hidden files as a small toggle, the sort.
local function listbar(ui)
  return row{ gap = 8, w = "full", key = "files-listbar",
    input{ id = "files-filter", value = ui.filter or "", placeholder = "Filter by name", on = "filter", grow = 1, min_w = 90 },
    checkbox{ "Hidden", value = ui.hidden == true, on = "hidden", key = "files-hidden" },
    ui.tab == "files" and menu{ ui.sort == "date" and "Date" or "Name", id = "files-sort", icon = "rows", dense = true,
      items = { { id = "name", label = "Sort by name" }, { id = "date", label = "Sort by date, newest first" } }, on = "sort" } or false,
  }
end

-- A folder of .sbxw-artifacts: one row with its file count, folded until
-- pressed, so a deep tree costs one row per folder shown.
local function folderview(r)
  return row{ gap = 8, w = "full", px = 8, py = 4, radius = "xs", key = "files-row-" .. r.id, on = "fold", arg = r.id, fill = r.cursor and "elevated" or nil,
    on_right = (not r.open) and "fold" or nil, on_left = r.open and "fold" or nil,
    (r.depth or 0) > 0 and spacer{ w = 12 * r.depth, grow = 0 } or false,
    icon{ r.open and "chevron-down" or "chevron-right", size = 10, color = "text-muted" },
    icon{ "folder", size = 12, color = "blue" },
    text{ r.name, size = "sm", truncate = true, grow = 1 },
    text{ r.count .. (r.count == 1 and " file" or " files"), size = "xs", color = "text-muted" },
  }
end

-- A folder of the listed folder: opens in place (a chevron, folded at
-- first, its count once read); its own button lists it.
local function dirview(r)
  local count = r.loading and "…" or (r.count and (r.count .. (r.count == 1 and " item" or " items")) or nil)
  return column{ w = "full", gap = 2,
    row{ gap = 8, w = "full", px = 8, py = 4, radius = "xs", key = "files-row-" .. r.id, on = "expand", arg = r.id, hover_group = true,
      on_right = (not r.open) and "expand" or nil, on_left = r.open and "expand" or nil,
      fill = r.cursor and "elevated" or nil, help = (r.open and "Fold " or "Open ") .. r.name .. " here (Right, Left)",
      (r.depth or 0) > 0 and spacer{ w = 12 * r.depth, grow = 0 } or false,
      icon{ r.open and "chevron-down" or "chevron-right", size = 10, color = "text-muted" },
      icon{ "folder", size = 12, color = "blue" },
      text{ r.name .. (r.link and " ↪" or ""), size = "sm", truncate = true, grow = 1 },
      count and text{ count, size = "xs", color = "text-muted" } or false,
      nz(r.when) and text{ r.when, size = "xs", color = "text-dim" } or false,
      button{ icon = "chevron-right", key = "files-into-" .. r.id, on = "select", arg = r.id, variant = "ghost", dense = true, reveal = true,
        help = "List " .. r.name .. " (Enter)" },
    },
    nz(r.error) and row{ px = 8, (r.depth or 0) >= 0 and spacer{ w = 12 * (r.depth + 1) + 18, grow = 0 } or false,
      text{ r.error, size = "xs", color = "red", wrap = true, grow = 1 } } or false,
  }
end

local function rowview(ui, r, i)
  if r.kind == "folder" then return folderview(r) end
  if r.kind == "dir" and ui.tab == "files" then return dirview(r) end
  local meta = {}
  if nz(r.dir) then meta[#meta + 1] = r.dir end
  if nz(r.when) then meta[#meta + 1] = r.when end
  local kids = { gap = 8, w = "full", px = 8, py = 4, radius = "xs", key = "files-row-" .. r.id, on = "select", arg = r.id,
    fill = r.selected and "active-bg" or (r.cursor and "elevated" or nil),
    (r.depth or 0) > 0 and spacer{ w = 12 * r.depth + 18, grow = 0 } or false,
    icon{ r.icon or "file", size = 12, color = r.kind == "dir" and "blue" or "text-muted" },
    column{ gap = 2, grow = 1,
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

local function rows(ui, flat)
  -- One keyboard list: Up and Down move between rows, Right opens a folder
  -- in place, Left folds it, Enter presses the row (typing filters).
  local list = { gap = 2, w = "full", group = "list", key = "files-rows" }
  local last
  for i, r in ipairs(ui.rows or {}) do
    if ui.tab == "recent" and r.group ~= last then
      last = r.group
      list[#list + 1] = row{ py = 4, head(r.group or "") }
    end
    list[#list + 1] = rowview(ui, r, i)
  end
  -- The explorer draws at most a page of rows (the core refuses a render
  -- past its element cap); the rest is counted, and the filter runs over all.
  local more = more_rows(ui.count, { key = "files-more", on = "more", hint = "Filter by name to find one" })
  if more then list[#list + 1] = more end
  local err = ui.errors and ui.errors[ui.tab]
  if nz(err) then
    local where = ui.tab == "files" and ("Cannot read " .. (ui.path or "this folder") .. ": ") or "Cannot read it: "
    list[#list + 1] = column{ gap = 4, w = "full", key = "files-error",
      note(where .. err, "red"),
      note("Pick another location at the left (Home is the agent's home, ~), or press ↑ for the folder above."),
    }
  elseif #(ui.rows or {}) == 0 then
    local loading = ui.loading and ui.loading[ui.tab]
    local empty = {
      artifacts = "No deliverables yet. Agents put documents, pictures and reports in .sbxw-artifacts/; they show here.",
      files = nz(ui.filter) and ("Nothing matches \"" .. ui.filter .. "\"" .. (ui.hidden and "" or ": clear the filter, or tick Hidden for dotfiles") .. ".")
        or ("Nothing here. Home is at ~ (" .. (ui.home or "/home/agent") .. "): press Home at the left."),
      recent = "Nothing written in the last " .. (ui.recent_days or 7) .. " days.",
    }
    list[#list + 1] = note(loading and "Loading…" or empty[ui.tab] or "", nil)
  end
  -- Narrow, the whole tree scrolls as one (the caller's scroll).
  if flat then return column(list) end
  return scroll{ column(list) }
end

local function preview(ui, narrow)
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
    -- About 70 characters a line, the prose face; links followed inside.
    local md = column{ w = "full", max_w = 640, gap = 8,
      nz(p.section ~= nil and "x" or "") and row{ gap = 8, button{ "Whole file", key = "files-whole", on = "outline", variant = "ghost", tone = "blue" } } or false,
      markdown{ p.text or "", on = "link", size = 14 }, nz(p.note) and note(p.note) or false }
    local outline = p.outline or {}
    if #outline > 1 and not narrow then
      local o = { gap = 2, w = 200, key = "files-outline", text{ "Outline", size = "caption", weight = "semibold", color = "text-muted" } }
      for _, h in ipairs(outline) do
        o[#o + 1] = row{ key = "files-outline-" .. h.index, on = "outline", arg = h.index, px = 4, h = 24, radius = "xs", hover = "hover-subtle",
          fill = h.on and "active-bg" or nil, help = h.text,
          spacer{ w = 8 * ((h.level or 1) - 1), grow = 0 },
          text{ h.text, size = "xs", color = h.on and "text-bright" or "text-muted", truncate = true, grow = 1 } }
      end
      body = row{ gap = 16, w = "full", h = "full", align = "start", scroll{ grow = 1, md }, scroll{ w = 200, h = "full", column(o) } }
    elseif #outline > 1 then
      -- Narrow: the outline is a menu above the text.
      local items = {}
      for _, h in ipairs(outline) do items[#items + 1] = { id = tostring(h.index), label = string.rep("  ", (h.level or 1) - 1) .. h.text } end
      body = column{ gap = 8, w = "full", h = "full",
        menu{ "Outline", id = "files-outline", icon = "rows", items = items, on = "outline_pick" },
        scroll{ md } }
    else
      body = scroll{ md }
    end
  elseif p.state == "text" or p.state == "html" then
    body = scroll{
      p.state == "html" and note("An HTML page, shown as its source: the app does not draw pages. Open it to see it in your browser.", "amber") or false,
      text{ p.numbered or p.text or "", mono = true, size = "xs", wrap = true }, nz(p.note) and note(p.note) or false }
  elseif p.state == "image" then
    body = column{ gap = 4, image{ key = p.key, max_w = 900, max_h = 600 }, note((p.w or "?") .. " × " .. (p.h or "?") .. " px") }
  elseif p.state == "error" then
    body = note(p.text or "Could not read it.", "red")
  else
    body = note(p.text or "")
  end
  body = column{ key = "files-preview-" .. (p.state == "html" and "text" or p.state or "note"), w = "full", grow = 1, body }
  return column{ gap = 8, h = "full", pad = 8,
    text{ s.name, weight = "semibold", truncate = true, color = "text-bright" },
    text{ s.type .. (isfile and (" · " .. s.size .. " · " .. s.modified) or ""), size = "xs", color = "text-muted", truncate = true },
    text{ s.path or "", size = "xs", mono = true, color = "text-dim", truncate = true, help = s.path },
    row(acts),
    divider{},
    body,
  }
end

return {
  init = function()
    -- Narrow (the overlay under 760 pt): the tree or the reader, one at a time.
    return { pane = "tree" }
  end,

  render = function(state, data)
    local ui = data.files_ui
    if not ui or not ui.sandbox then
      return column{ pad = 12, note("Pick a sandbox: the rail's Files, or Project on a pane's bar.") }
    end
    local over = (data.app or {}).overlay or {}
    local narrow = (over.w or 1000) < 760
    local dragging = over.drop_over == true
    local open_up = state.upload or dragging or #(ui.staged or {}) > 0 or ui.saved ~= nil
    -- The header, across both columns: where you are, and the secondary actions.
    local actions = {
      { id = "refresh", label = "Read the lists again", icon = "reload" },
      { id = "codemap", label = ui.has_map and "Code map" or "Generate code map", icon = "sparkle" },
    }
    local head = row{ gap = 8, w = "full", key = "files-head",
      crumbs(state, ui, narrow),
      button{ (not narrow) and "Upload" or nil, key = "files-upload", icon = "plus", on = "upload", variant = open_up and "neutral" or "ghost", dense = true,
        help = "Upload into " .. ui.sandbox .. ": drop files on the explorer, choose them or paste a picture" },
      menu{ "", id = "files-actions", icon = "more", items = actions, on = "action", arg = { sandbox = ui.sandbox, has_map = ui.has_map == true } },
    }
    local top = { gap = 8, w = "full", head }
    if open_up then top[#top + 1] = uploads(ui, dragging) end
    local three = (over.w or 1000) >= 900
    local picker = row{ gap = 8, w = "full",
      sandbox_picker(data, { id = "files-sandbox", value = ui.sandbox, items = ui.sandboxes or {}, on = "sandbox" }),
      spacer{},
      button{ icon = "reload", key = "files-refresh", on = "refresh", variant = "ghost", dense = true, help = "Read the lists again" } or false,
      ui.offline and badge{ "stopped: workspace only", tone = "amber", help = "Read from this computer; the sandbox is not started to list a folder" } or false,
    }
    local list_head = nz(ui.artifacts_dir) and ui.tab == "artifacts" and text{ ui.artifacts_dir, size = "xs", color = "text-dim", mono = true, truncate = true, help = ui.artifacts_dir } or false
    if narrow then
      if state.pane == "reader" and ui.selected then
        return column{ h = "full", gap = 8, w = "full",
          row{ w = "full", button{ "← Files", key = "files-to-tree", on = "to_tree", variant = "ghost", tone = "blue", help = "Back to the list" }, spacer{} },
          preview(ui, true) }
      end
      top[#top + 1] = scroll{ key = "files-tree-scroll", column{ gap = 8, w = "full",
        picker, locations(ui, true), list_head, listbar(ui), rows(ui, true) } }
      top.h = "full"
      return column(top)
    end
    -- From 900 pt: three columns, the Locations on their own at the left
    -- (scrolling on their own, the picker above), the tree the full height,
    -- the reader. From 760: two, the Locations folded into one line.
    local tree = column{ h = "full", gap = 8, w = "full",
      (not three) and picker or false,
      (not three) and locations(ui, true) or false,
      list_head,
      listbar(ui),
      rows(ui),
    }
    local reading = split{ id = "files-split", vertical = true, ratio = three and 0.44 or 0.4, tree, preview(ui) }
    if three then
      top[#top + 1] = row{ gap = 12, w = "full", grow = 1, align = "stretch", key = "files-columns",
        column{ w = 224, h = "full", gap = 8, key = "files-left",
          picker,
          scroll{ key = "files-locations-scroll", locations(ui, false) } },
        divider{ vertical = true },
        column{ grow = 1, min_w = 0, h = "full", reading } }
    else
      top[#top + 1] = reading
    end
    top.h = "full"
    return column(top)
  end,
  actions = {
    tab = function(_, t) return intent("files_tab", { tab = t }) end,
    upload = function(state) state.upload = not state.upload end,
    action = function(_, a, id)
      if id == "refresh" then return intent("files_refresh", {}) end
      if id == "codemap" and a then
        if a.has_map then return intent("codemap_open", { sandbox = a.sandbox }) end
        return intent("codemap_generate", { sandbox = a.sandbox })
      end
    end,
    goto = function(state) state.goto = not state.goto end,
    crumb = function(_, path) if path then return intent("files_browse", { path = path }) end end,
    crumb_pick = function(_, _, path) if path and path ~= "" then return intent("files_browse", { path = path }) end end,
    copy_folder = function(_, path) return intent("copy_text", { text = path }) end,
    expand = function(_, id) return intent("files_expand", { id = id }) end,
    sort = function(_, _, by) return intent("files_sort", { by = by }) end,
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
    go = function(state, _, path) if path and path ~= "" then state.goto = false; return intent("files_browse", { path = path }) end end,
    star = function(_, p) return intent("files_star", { path = p }) end,
    -- A Locations row passes its id as its arg; the narrow menu as its value.
    place = function(_, a, v)
      local id = (type(v) == "string" and v ~= "") and v or a
      if id then return intent("files_place", { id = id }) end
    end,
    hidden = function(_, _, on) return intent("files_hidden", { on = on }) end,
    filter = function(_, _, t) return intent("files_filter", { text = t or "" }) end,
    more = function() return intent("files_more", {}) end,
    fold = function(_, id) return intent("files_fold", { id = id }) end,
    select = function(state, id) state.pane = "reader"; return intent("files_select", { id = id }) end,
    to_tree = function(state) state.pane = "tree" end,
    outline_pick = function(_, _, id) return intent("files_outline", { index = tonumber(id) }) end,
    show = function(_, id) return intent("files_show_in_folder", { id = id }) end,
    download = function() return intent("files_download", {}) end,
    reveal = function() return intent("files_reveal", {}) end,
    open_with = function() return intent("files_open_with", {}) end,
    save_as = function() return intent("files_save_as", {}) end,
    copy = function() return intent("files_copy_path", {}) end,
    delete = function() return intent("files_delete", {}) end,
    link = function(_, _, url) return intent("files_link", { url = url }) end,
    worktree = function(_, a) return intent("files_open", { sandbox = a.sandbox, worktree = a.thread }) end,
    sandbox = function(_, _, id) if id and id ~= "" then return intent("files_open", { sandbox = id }) end end,
    outline = function(_, i) if i ~= nil then return intent("files_outline", { index = i }) end return intent("files_outline", {}) end,
  },
}
