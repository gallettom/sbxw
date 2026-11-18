-- Kits (an overlay over the pane area, opened from the rail; it was Setup's
-- Kits tab): the kits waiting for your answer on a running sandbox, and the
-- kits line of each sbxw.toml sbxw uses. The web UI's kits.js. Wide (760 pt
-- and more): the sandbox picker, what waits for you and the kits line at the
-- left, the selected kit's description at the right. Narrow: one column,
-- the description under the list.
--
-- The file shown: data.kits_view.path, else the file data.kits_view.sandbox
-- uses (the panel opened for a sandbox), else the last one used
-- (data.app.kept.kits), else the daemon's own file. Picking another asks
-- `kits_pick`. The line is edited here; saving it is the core's dialog
-- (`kits_save`), which asks again for a kit that reaches out. A kit waiting
-- is answered in sbx's own prompt, in a pane: never here.

-- Rows drawn at a time; past this, "Show N more".
local PAGE = 25

local function failed(v)
  if type(v) == "table" and v.ok == false then return v.error or "the daemon refused" end
  return nil
end

local function trim(v) return (string.gsub(v or "", "^%s*(.-)%s*$", "%1")) end

local function contains(list, v)
  for _, x in ipairs(list or {}) do if x == v then return true end end
  return false
end

local function note(t, color)
  return text{ t, wrap = true, size = "sm", color = color or "text-muted" }
end

local function section(t, extra)
  return row{ w = "full", gap = 8,
    text{ t, size = "caption", weight = "semibold", color = "text-muted" },
    extra or false,
  }
end

local function kit_label(ref)
  if string.find(ref, "/kits/jarvis%-") then return "Jarvis kit" end
  if string.find(ref, "^[%./~]") then return "local kit" end
  return false
end

-- The file shown (see the top).
local function pick_file(files, view, kept)
  local function by_path(p)
    for _, f in ipairs(files) do if p and f.path == p then return f end end
  end
  local function by_sandbox(sb)
    for _, f in ipairs(files) do if sb and contains(f.sandboxes, sb) then return f end end
  end
  local f = by_path(view.path) or by_sandbox(view.sandbox) or by_path(kept.path) or by_sandbox(kept.sandbox)
  if f then return f end
  for _, x in ipairs(files) do if x.daemon then return x end end
  return files[1]
end

-- The line being edited: the person's edits, or the file's own line.
local function kits_list(k, state, f)
  if state.edit and state.edit.path == f.path then return state.edit.list end
  if f.kits then return f.kits end
  return k.defaults or {}
end

-- The selected kit's description (the right column; under the picker when
-- narrow), or nil when none is selected.
local function describe(ref, bundled, cur, f)
  if not ref then return nil end
  local b = nil
  for _, x in ipairs(bundled) do if x.ref == ref then b = x end end
  local lab = kit_label(ref) or (b and "bundled with sbxw") or (string.sub(ref, 1, 5) == "sbxw:" and "bundled with sbxw") or "registry kit"
  local on = contains(cur, ref)
  local users = {}
  for _, n in ipairs(f.sandboxes or {}) do users[#users + 1] = n end
  return column{ w = "full", gap = 8, pad = 12, radius = "md", border = "elevated", key = "kits-about",
    row{ w = "full", gap = 8,
      text{ ref, mono = true, weight = "semibold", color = "text-bright", truncate = true, grow = 1, help = ref },
      badge{ lab, tone = lab == "Jarvis kit" and "purple" or "neutral" },
      (b and b.default) and badge{ "default", help = "What a file without a kits line gets" } or false,
      button{ icon = "close", variant = "ghost", on = "sel", key = "kits-about-close", help = "Hide" },
    },
    note(on and ("In this kits line: " .. (#users > 0 and table.concat(users, ", ") or "its sandboxes") .. " get it at their next up.") or "Not in this kits line: tick it at the left, then Save.", on and "green" or "text-muted"),
    text{ (b and b.about) or (lab == "local kit" and "A kit folder on this machine: its kit.yaml says what it adds." or lab == "Jarvis kit" and "A kit Jarvis wrote for this project." or "A kit from a registry, fetched by sbx when the sandbox starts."),
      wrap = true, color = "text-soft" },
  }
end

return {
  init = function() return { edit = nil, add = "", more = 0, sel = nil } end,

  render = function(state, data)
    local errors = (data.app and data.app.errors) or {}
    local k = data.kits or {}
    local err = errors.kits or failed(data.kits)
    local view = data.kits_view or {}
    local intro = text{ "A kit is what a sandbox starts with on top of its agent: tools, skills, network rules. Each project lists its kits in the kits line of its sbxw.toml; a change reaches a sandbox at its next up.", wrap = true, color = "text-soft", key = "kits-intro" }
    local wide = overlay_wide(data)
    -- Wide, the sentence heads the right column (the list keeps the height).
    local out = { w = "full", gap = 12, key = "kits-body", (not wide) and intro or false }
    if err then
      out[#out + 1] = row{ w = "full", gap = 8, key = "kits-error",
        icon{ "warning", size = 12, color = "red" },
        text{ err, wrap = true, size = "sm", color = "red", grow = 1 },
        button{ "Read again", icon = "reload", variant = "ghost", on = "reload", key = "kits-reload" },
      }
    end

    -- Waiting for you: sbx asks before a kit with network rules reaches a
    -- running sandbox; the answer is given in its prompt, in a pane.
    local waiting = {}
    for _, sb in ipairs(data.sandboxes or {}) do
      if (sb.kits_waiting or 0) > 0 then
        waiting[#waiting + 1] = row{ w = "full", gap = 8, key = "waiting-" .. sb.name,
          icon{ "warning", size = 12, color = "amber" },
          text{ sb.name, weight = "medium", color = "text-bright" },
          text{ sb.kits_waiting .. (sb.kits_waiting == 1 and " kit waits" or " kits wait") .. " for your answer", size = "sm", color = "text-muted", truncate = true, grow = 1 },
          button{ "Answer in a pane", icon = "terminal", on = "kit_add", arg = sb.name, key = "kit-answer-" .. sb.name,
            help = "Opens sbx kit add in a pane: answer sbx's prompt there, one kit per answer" },
        }
      end
    end
    if #waiting > 0 then
      out[#out + 1] = column{ w = "full", gap = 8, pad = 12, radius = "md", border = "amber", fill = "surface", key = "kits-waiting",
        section("Waiting for you", badge{ tostring(#waiting), tone = "amber" }),
        note("A kit that adds network rules makes sbx ask you before it reaches a running sandbox."),
        list(waiting),
      }
    end

    local files = k.files or {}
    if #files == 0 then
      if not err then
        out[#out + 1] = note("No sbxw.toml to show: the daemon runs without one and no sandbox records one. Start the daemon from a project with sbxw web, or create a sandbox from a project folder; its kits line then shows here.")
      end
      if wide then table.insert(out, 1, intro) end
      return column{ h = "full", w = "full", scroll{ gap = 0, column(out) } }
    end
    local kept = ((data.app or {}).kept or {}).kits or {}
    local f = pick_file(files, view, kept)
    -- The sandbox picker at the top: the sandbox asked for, else the first
    -- one the file shown serves, else the last one used.
    local sbs, sb_ids = {}, {}
    for _, sb in ipairs(data.sandboxes or {}) do sbs[#sbs + 1] = { id = sb.name, label = sb.name }; sb_ids[#sb_ids + 1] = sb.name end
    local on_sb = view.sandbox
    if not contains(sb_ids, on_sb) then on_sb = (f.sandboxes or {})[1] end
    if not contains(sb_ids, on_sb) then on_sb = kept.sandbox end
    if not contains(sb_ids, on_sb) then on_sb = nil end
    -- Beside the overlay's title (the core's header draws it: TITLE_SLOT).
    table.insert(out, 1, row{ key = "title-slot", gap = 8, text{ "Sandbox", size = "sm", color = "text-muted" },
      sandbox_picker(data, { id = "kits-sandbox", value = on_sb, items = sbs, on = "pick_sandbox", placeholder = "No sandbox", disabled = #sbs == 0 }) })
    local items = {}
    for _, x in ipairs(files) do items[#items + 1] = { id = x.path, label = x.path } end
    local cur = kits_list(k, state, f)
    local broken = f.error ~= nil
    local dirty = state.edit ~= nil and state.edit.path == f.path

    local users = {}
    if f.daemon then users[#users + 1] = "the daemon (new sandboxes from this app)" end
    for _, n in ipairs(f.sandboxes or {}) do users[#users + 1] = n end
    local lines = {}
    if #users > 0 then lines[#lines + 1] = "Used by " .. table.concat(users, ", ") .. "." end
    if not f.exists then lines[#lines + 1] = "The file does not exist yet: saving creates it." end
    if not f.kits then lines[#lines + 1] = "No kits line: the default kits apply. Saving writes them out." end

    out[#out + 1] = section("Kits line", dirty and badge{ "unsaved", tone = "amber" } or false)
    out[#out + 1] = select{ id = "kits-file", value = f.path, items = items, on = "pick" }
    if #lines > 0 then out[#out + 1] = note(table.concat(lines, " ")) end
    if broken then out[#out + 1] = note("Cannot be edited: " .. f.error, "red") end

    local bundled = {}
    for _, b in ipairs(k.bundled or {}) do
      local on = contains(cur, b.ref)
      bundled[#bundled + 1] = row{ w = "full", gap = 8,
        checkbox{ b.ref, value = on, on = "toggle", arg = { ref = b.ref, path = f.path, list = cur }, disabled = broken, key = "kit-" .. b.name },
        row{ gap = 8, grow = 1, min_w = 0, px = 4, h = 28, radius = "xs", key = "kit-sel-" .. b.name, on = "sel", arg = b.ref,
          fill = state.sel == b.ref and "active-bg" or nil, hover = "hover-subtle", help = "What " .. b.ref .. " gives a sandbox",
          b.default and badge{ "default", help = "What a file without a kits line gets" } or false,
          text{ b.about or "", size = "sm", color = on and "text-soft" or "text-dim", truncate = true, grow = 1 },
        },
      }
    end
    out[#out + 1] = section("Bundled with sbxw")
    out[#out + 1] = list(bundled)

    local others = {}
    for _, ref in ipairs(cur) do
      if string.sub(ref, 1, 5) ~= "sbxw:" then others[#others + 1] = ref end
    end
    local shown = PAGE + (state.more or 0)
    local rows = {}
    for i, ref in ipairs(others) do
      if i > shown then break end
      local lab = kit_label(ref)
      rows[#rows + 1] = row{ w = "full", gap = 8,
        row{ gap = 8, grow = 1, min_w = 0, px = 4, h = 28, radius = "xs", key = "kit-sel-" .. ref, on = "sel", arg = ref,
          fill = state.sel == ref and "active-bg" or nil, hover = "hover-subtle", help = ref,
          dot{ tone = "green" },
          text{ ref, mono = true, truncate = true, grow = 1 },
          lab and badge{ lab, tone = lab == "Jarvis kit" and "purple" or "neutral" } or false,
        },
        button{ "Remove", variant = "ghost", on = "remove", arg = { ref = ref, path = f.path, list = cur }, disabled = broken, key = "kit-rm-" .. ref },
      }
    end
    out[#out + 1] = section("Other kits", #others > 0 and badge{ tostring(#others) } or false)
    if #rows > 0 then
      out[#out + 1] = list(rows)
      if #others > shown then
        local n = math.min(PAGE, #others - shown)
        out[#out + 1] = row{ w = "full", note("Showing " .. shown .. " of " .. #others .. "."), spacer{},
          button{ "Show " .. n .. " more", variant = "ghost", on = "more", key = "kits-more" } }
      end
    else
      out[#out + 1] = note("None: only bundled kits. Add a local kit's path or a registry reference below.")
    end
    out[#out + 1] = row{ w = "full", gap = 8,
      input{ id = "kits-add", value = state.add or "", on = "typed", on_submit = "push", arg = { path = f.path, list = cur },
        placeholder = "A path or a registry reference", disabled = broken },
      button{ "Add", icon = "plus", key = "kit-push", on = "push", arg = { path = f.path, list = cur }, disabled = broken },
    }
    -- The line's actions stay in view under the list (the panel scrolls above).
    local footer = row{ w = "full", gap = 8,
      button{ "Use defaults", key = "kits-defaults", variant = "ghost", on = "defaults", arg = f.path, disabled = broken or not f.kits,
        help = "Remove the kits line: its sandboxes then get the default kits" },
      spacer{},
      dirty and button{ "Discard", variant = "ghost", on = "discard", key = "kits-discard" } or false,
      button{ "Save…", key = "kits-save", variant = "primary", on = "save", disabled = broken or not dirty },
    }
    -- The selected kit: what it is and what it gives a sandbox.
    local about = describe(state.sel, k.bundled or {}, cur, f)
    local left = column{ h = "full", w = "full", gap = 8, scroll{ gap = 0, column(out) }, divider{}, footer }
    if not wide then
      if about then table.insert(out, 2, about) end
      return left
    end
    return split{ id = "kits-split", vertical = true, ratio = 0.56, left,
      column{ h = "full", w = "full", px = 4, scroll{ gap = 0, column{ w = "full", gap = 12,
        intro,
        about or column{ w = "full", gap = 8, key = "kits-about-none",
          text{ "What a kit gives", size = "caption", weight = "semibold", color = "text-muted" },
          note("Press a kit's description at the left to read it in full here. Ticking a kit puts it in the kits line; Save writes the line."),
        } } } } }
  end,

  actions = {
    reload = function() return intent("kits_show") end,
    pick = function(state, _, path) state.edit = nil; return intent("kits_pick", { path = path }) end,
    pick_sandbox = function(state, _, sb) if sb and sb ~= "" then state.edit = nil; return intent("kits_pick", { sandbox = sb }) end end,
    sel = function(state, ref) if state.sel == ref or ref == nil then state.sel = nil else state.sel = ref end end,
    more = function(state) state.more = (state.more or 0) + PAGE end,
    toggle = function(state, a, on)
      local list = {}
      for _, k in ipairs(a.list or {}) do
        if k ~= a.ref then list[#list + 1] = k end
      end
      if on then list[#list + 1] = a.ref end
      state.edit = { path = a.path, list = list }
    end,
    remove = function(state, a)
      local list = {}
      for _, k in ipairs(a.list or {}) do
        if k ~= a.ref then list[#list + 1] = k end
      end
      state.edit = { path = a.path, list = list }
    end,
    typed = function(state, _, v) state.add = v end,
    push = function(state, a)
      local ref = trim(state.add)
      if ref == "" then return end
      local list = {}
      for _, k in ipairs(a.list or {}) do list[#list + 1] = k end
      if not contains(list, ref) then list[#list + 1] = ref end
      state.edit = { path = a.path, list = list }
      state.add = ""
    end,
    discard = function(state) state.edit = nil end,
    save = function(state)
      if not state.edit then return end
      local e = state.edit
      return intent("kits_save", { path = e.path, kits = table.concat(e.list, "\n") })
    end,
    defaults = function(state, path)
      state.edit = nil
      return intent("kits_save", { path = path, defaults = true })
    end,
    kit_add = function(_, name)
      return intent("open_terminal", { sandbox = name, mode = "kit-add", pane = "spare" })
    end,
  },
}
