-- Skills (an overlay over the pane area, opened from the rail; it was
-- Setup's Skills tab): the skills you keep for every new sandbox, and
-- keeping one a sandbox's agent wrote. The web UI's skills.js. One column
-- at every width: the picked sandbox's skills to look at and keep on top
-- (the action), your kept skills under it (the list).
--
-- The picker's sandbox: data.skills_view.sandbox (the panel opened for a
-- sandbox), else the last one used (data.app.kept.skills.sandbox), else the
-- first sandbox. Picking another asks `skills_pick`; looking stays a press
-- (`skills_look` starts a stopped sandbox). Keeping and removing are the
-- core's dialogs. After Look, "Installed in <sandbox>" lists every skill and
-- command it has (`data.sandbox_commands`, the daemon's /commands route),
-- grouped by origin and read only: kits and built-ins come back with every
-- sandbox, so they have no Keep.

-- Rows drawn at a time; past this, "Show N more".
local PAGE = 25

local function failed(v)
  if type(v) == "table" and v.ok == false then return v.error or "the daemon refused" end
  return nil
end

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

local function kb(n)
  n = tonumber(n) or 0
  if n < 1024 then return n .. " B" end
  return math.floor(n / 1024 + 0.5) .. " KB"
end

-- The first PAGE of `rows` (plus `extra` more), and the "Show more" row.
local function paged(rows, extra, key)
  local shown = PAGE + (extra or 0)
  local out = {}
  for i, r in ipairs(rows) do
    if i > shown then break end
    out[#out + 1] = r
  end
  local more = false
  if #rows > shown then
    local n = math.min(PAGE, #rows - shown)
    more = row{ w = "full", note("Showing " .. shown .. " of " .. #rows .. "."), spacer{},
      button{ "Show " .. n .. " more", variant = "ghost", on = "more", arg = key, key = "skills-more-" .. key } }
  end
  return list(out), more
end

-- A sandbox text cut to `n` characters with an ellipsis (agent or sandbox
-- data: never trusted to be short).
local function clip(t, n)
  t = tostring(t or "")
  local len = utf8.len(t)
  if not len then return "?" end
  if len <= n then return t end
  return string.sub(t, 1, utf8.offset(t, n) - 1) .. "…"
end

-- Where an entry of `GET /api/sandboxes/:name/commands` comes from, in plain
-- words: { id, title, where }. The route says workspace, home or built-in;
-- home is ~/.claude/commands (kits, kept skills) or ~/.claude/skills (sbx's
-- shared store, or a skill the agent wrote there: the keepable list names
-- those). A `kit:<name>` origin names its kit.
local function origin_of(c, kept, own_home)
  local o = tostring(c.origin or "")
  if o == "built-in" then return { id = "built-in", title = "Built-in", where = "Claude Code's own commands" } end
  if o == "workspace" then return { id = "project", title = "Project", where = "the project's .claude/skills/ and .claude/commands/" } end
  local kit = string.match(o, "^kit:(.+)$")
  if kit then return { id = "kit-" .. clip(kit, 40), title = "Kit " .. clip(kit, 40), where = "installed by that kit" } end
  if o == "home" and c.kind == "command" then
    if kept[c.name] then return { id = "kept", title = "Your kept skills", where = "the sbxw-my-skills kit" } end
    return { id = "kits", title = "Kits", where = "~/.claude/commands/, written by the sandbox's kits" }
  end
  if o == "home" then
    if own_home[c.name] then return { id = "own", title = "Written in this sandbox", where = "~/.claude/skills/: you can keep it above" } end
    return { id = "store", title = "sbx shared skills", where = "~/.claude/skills/, sbx's shared skills store" }
  end
  return { id = "other-" .. clip(o, 24), title = clip(o, 24), where = "" }
end

-- The order the groups show in: what kits bring first, built-in last.
local GROUP_ORDER = { kits = 1, kept = 2, store = 3, own = 4, project = 5, ["built-in"] = 7 }

-- "Installed in <sandbox>": every skill and command it has (from
-- `data.sandbox_commands`), grouped by origin, read only. Nil when the
-- daemon has no such route or the read failed (the panel is as before).
local function installed(state, data, look_for, kept, found)
  local cmds = data.sandbox_commands
  if type(cmds) ~= "table" or type(cmds.commands) ~= "table" then return nil end
  local own_home = {}
  if type(found) == "table" then
    for _, x in ipairs(found.skills or {}) do
      if x.origin == "home" then own_home[x.name] = true end
    end
  end
  local groups, order = {}, {}
  for _, c in ipairs(cmds.commands) do
    if type(c) == "table" and type(c.name) == "string" then
      local o = origin_of(c, kept, own_home)
      local g = groups[o.id]
      if not g then
        g = { o = o, rows = {} }
        groups[o.id] = g
        order[#order + 1] = o.id
      end
      local desc = type(c.description) == "string" and c.description ~= "" and c.description or nil
      g.rows[#g.rows + 1] = row{ w = "full", gap = 8, align = "start", key = "installed-" .. clip(c.name, 64),
        dot{ tone = "neutral" },
        column{ grow = 1, gap = 2,
          text{ "/" .. clip(c.name, 64), mono = true, color = "text-bright" },
          desc and text{ clip(desc, 160), size = "sm", wrap = true, color = "text-soft" } or false,
        },
        text{ c.kind == "command" and "command" or "skill", size = "xs", color = "text-muted" },
      }
    end
  end
  table.sort(order, function(a, b)
    local function rank(id) return GROUP_ORDER[id] or (string.sub(id, 1, 4) == "kit-" and 0 or 6) end
    local ra, rb = rank(a), rank(b)
    if ra ~= rb then return ra < rb end
    return a < b
  end)
  local total = 0
  for _, id in ipairs(order) do total = total + #groups[id].rows end
  local out = { w = "full", gap = 10, key = "skills-installed" }
  out[#out + 1] = row{ w = "full", gap = 8,
    text{ "Installed in " .. look_for, size = "caption", weight = "semibold", color = "text-muted", truncate = true, grow = 1 },
    badge{ total .. " installed", tone = "neutral", key = "skills-installed-count" },
  }
  out[#out + 1] = note("What every sandbox gets already, read only: you type them after /. Nothing here needs keeping.")
  if cmds.complete == false then
    out[#out + 1] = row{ w = "full", key = "skills-installed-reason",
      note(clip(cmds.reason or "The list is partial.", 200)) }
  end
  for _, id in ipairs(order) do
    local g = groups[id]
    local gc = { w = "full", gap = 6, key = "installed-group-" .. id }
    gc[#gc + 1] = row{ w = "full", gap = 8,
      text{ g.o.title, size = "sm", weight = "semibold", color = "text-soft" },
      badge{ tostring(#g.rows), tone = "neutral" },
      g.o.where ~= "" and text{ g.o.where, size = "xs", color = "text-muted", truncate = true, grow = 1 } or false,
    }
    local l, more = paged(g.rows, state.more["inst-" .. id], "inst-" .. id)
    gc[#gc + 1] = l
    gc[#gc + 1] = more
    out[#out + 1] = column(gc)
  end
  return column(out)
end

return {
  init = function() return { more = {} } end,

  render = function(state, data)
    local errors = (data.app and data.app.errors) or {}
    local ms = data.my_skills or {}
    local kept_names = {}
    for _, x in ipairs(ms.skills or {}) do kept_names[x.name] = true end
    -- The sandbox to look into (the picker, at the top left).
    local names, ids = {}, {}
    for _, sb in ipairs(data.sandboxes or {}) do
      names[#names + 1] = { id = sb.name, label = sb.name .. (sb.status == "running" and "" or " (" .. (sb.status or "?") .. ")") }
      ids[#ids + 1] = sb.name
    end
    local want = (data.skills_view or {}).sandbox or ((((data.app or {}).kept or {}).skills or {}).sandbox)
    local pick = contains(ids, want) and want or ids[1]
    local wide = overlay_wide(data)
    local out = { w = "full", gap = 12, key = "skills-body",
      -- Beside the overlay's title (the core's header draws it: TITLE_SLOT).
      row{ key = "title-slot", gap = 8,
        select{ id = "skills-sandbox", value = pick or "", on = "pick", scope = "sandbox", placeholder = "No sandbox", disabled = #names == 0 } },
      row{ w = "full", gap = 4, key = "skills-about",
        help = "A skill is a set of instructions an agent can follow (a /command). Skills you keep here are given to every sandbox created from now on. A sandbox can write a skill, never make it reach the others: keeping one is your act.",
        icon{ "info", size = 12, color = "text-muted" },
        text{ "Keep a skill a sandbox's agent wrote: every new sandbox gets it.", size = "sm", color = "text-muted", truncate = true, grow = 1 } },
    }
    local err = errors.my_skills or failed(data.my_skills)
    if err then
      out[#out + 1] = row{ w = "full", gap = 8, key = "skills-error",
        icon{ "warning", size = 12, color = "red" },
        text{ err, wrap = true, size = "sm", color = "red", grow = 1 },
        button{ "Read again", icon = "reload", variant = "ghost", on = "reload", key = "skills-reload" },
      }
    end

    local kept = {}
    for _, x in ipairs(ms.skills or {}) do
      kept[#kept + 1] = row{ w = "full", gap = 8, align = "start", key = "kept-" .. x.name,
        dot{ tone = "green" },
        column{ grow = 1, gap = 2,
          text{ "/" .. x.name, mono = true, color = "text-bright" },
          x.description and text{ x.description, size = "sm", wrap = true, color = "text-soft" } or false,
          text{ "from " .. (x.sandbox or "?"), size = "xs", color = "text-muted", help = x.path },
        },
        button{ "Remove", variant = "ghost", on = "forget", arg = x.name, key = "forget-" .. x.name, help = "New sandboxes stop getting it" },
      }
    end
    local yours = { w = "full", gap = 12, key = "skills-kept" }
    yours[#yours + 1] = section("Your skills", badge{ #kept .. " kept", tone = #kept > 0 and "green" or "neutral" })
    if #kept > 0 then
      local l, more = paged(kept, state.more.kept, "kept")
      yours[#yours + 1] = l
      yours[#yours + 1] = more
    elseif not err then
      yours[#yours + 1] = note("None kept yet. Press Look above on a sandbox: you can keep any skill its agent wrote.")
    end

    -- One column: keep a skill from a sandbox on top (the action), your
    -- skills under it (the list), the whole width.
    local main = out
    out = { w = "full", gap = 12, key = "skills-found", px = 12, py = 12, radius = "md", fill = "surface", border = "elevated" }
    out[#out + 1] = row{ w = "full", gap = 8,
      text{ pick and ("Skills in " .. pick) or "Keep a skill from a sandbox", size = "caption", weight = "semibold", color = "text-muted", truncate = true, grow = 1 },
      button{ "Look", icon = "search", variant = "primary", on = "look", arg = pick, disabled = not pick, key = "skills-look",
        help = "Lists the skills its agent wrote, which you can keep, and everything it has installed (a stopped sandbox is started)" },
    }
    if pick and data.skills_for ~= pick then
      out[#out + 1] = note("Press Look to list the skills " .. pick .. "'s agent wrote; keep one and every new sandbox gets it.")
    end
    if #names == 0 then
      out[#out + 1] = note("No sandbox yet: create one, let its agent write a skill, then keep it here.")
    end

    local look_for = data.skills_for
    if look_for and look_for == pick then
      local found = data.sandbox_skills
      if found == nil and errors.sandbox_skills then
        out[#out + 1] = note("Could not read " .. look_for .. "'s skills: " .. errors.sandbox_skills, "red")
      elseif found == nil then
        out[#out + 1] = note("Reading " .. look_for .. "'s skills… (a stopped sandbox is started)")
      elseif type(found) == "table" and found.ok == false then
        out[#out + 1] = note("Could not read " .. look_for .. "'s skills: " .. (found.error or "the daemon refused"), "red")
      elseif type(found) == "table" then
        local rows = {}
        for _, x in ipairs(found.skills or {}) do
          local is_kept = kept_names[x.name] == true
          rows[#rows + 1] = row{ w = "full", gap = 8, align = "start", key = "found-" .. x.name,
            dot{ tone = is_kept and "green" or (x.keepable and "blue" or "neutral") },
            column{ grow = 1, gap = 2,
              text{ "/" .. x.name, mono = true, color = "text-bright" },
              x.description and text{ x.description, size = "sm", wrap = true, color = "text-soft" } or false,
              text{ x.path .. " · " .. (x.files or 0) .. " files, " .. kb(x.bytes), size = "xs", color = "text-muted" },
            },
            x.keepable and button{ is_kept and "Replace…" or "Keep…", key = "keep-" .. x.name, on = "keep", arg = { sandbox = look_for, name = x.name },
              help = "Every sandbox created from now on gets it" }
              or text{ "name not kept", size = "sm", color = "text-muted", help = "Lowercase letters, digits, - and _ only" },
          }
        end
        local inst = installed(state, data, look_for, kept_names, found)
        if #rows > 0 then
          local l, more = paged(rows, state.more.found, "found")
          out[#out + 1] = l
          out[#out + 1] = more
        elseif inst then
          out[#out + 1] = note("Nothing to keep: no skill the agent wrote under the project's .claude/skills/ or the sandbox's own ~/.claude/skills/. What " .. look_for .. " has already is listed below.")
        else
          out[#out + 1] = note("No skill in " .. look_for .. ": none under the project's .claude/skills/ or the sandbox's own ~/.claude/skills/.")
        end
        out[#out + 1] = inst or false
      end
    end
    main[#main + 1] = column(out)
    main[#main + 1] = column(yours)
    local _ = wide
    return column{ h = "full", w = "full", scroll{ gap = 0, column{ w = "full", px = 4, column(main) } } }
  end,

  actions = {
    reload = function() return intent("skills_show") end,
    more = function(state, key) state.more[key] = (state.more[key] or 0) + PAGE end,
    pick = function(_, _, v) return intent("skills_pick", { sandbox = v }) end,
    look = function(_, name) return intent("skills_look", { sandbox = name }) end,
    keep = function(_, a) return intent("skill_keep", { sandbox = a.sandbox, name = a.name }) end,
    forget = function(_, name) return intent("skill_forget", { name = name }) end,
  },
}
