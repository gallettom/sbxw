-- Skills (an overlay over the pane area, opened from the rail; it was
-- Setup's Skills tab): the skills you keep for every new sandbox, and
-- keeping one a sandbox's agent wrote. The web UI's skills.js. Wide (760 pt
-- and more): the sandbox picker and the kept skills at the left, that
-- sandbox's skills to look at and keep at the right. Narrow: one column.
--
-- The picker's sandbox: data.skills_view.sandbox (the panel opened for a
-- sandbox), else the last one used (data.app.kept.skills.sandbox), else the
-- first sandbox. Picking another asks `skills_pick`; looking stays a press
-- (`skills_look` starts a stopped sandbox). Keeping and removing are the
-- core's dialogs.

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
      text{ "A skill is a set of instructions an agent can follow (a /command). Skills you keep here are given to every sandbox created from now on. A sandbox can write a skill, never make it reach the others: keeping one is your act.", wrap = true, color = "text-soft" },
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
          text{ "from " .. (x.sandbox or "?"), size = "xs", color = "text-dim", help = x.path },
        },
        button{ "Remove", variant = "ghost", on = "forget", arg = x.name, key = "forget-" .. x.name, help = "New sandboxes stop getting it" },
      }
    end
    out[#out + 1] = section("Your skills", badge{ #kept .. " kept", tone = #kept > 0 and "green" or "neutral" })
    if #kept > 0 then
      local l, more = paged(kept, state.more.kept, "kept")
      out[#out + 1] = l
      out[#out + 1] = more
    elseif not err then
      out[#out + 1] = note("None kept yet. Pick a sandbox below and press Look: you can keep any skill its agent wrote.")
    end

    -- Keep a skill from a sandbox (the right column when wide).
    local main = out
    out = { w = "full", gap = 12, key = "skills-found" }
    if not wide then main[#main + 1] = divider{} end
    out[#out + 1] = row{ w = "full", gap = 8,
      text{ pick and ("Skills in " .. pick) or "Keep a skill from a sandbox", size = "caption", weight = "semibold", color = "text-muted", truncate = true, grow = 1 },
      button{ "Look", icon = "search", variant = "primary", on = "look", arg = pick, disabled = not pick, key = "skills-look",
        help = "Lists the skills it has under .claude/skills (a stopped sandbox is started)" },
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
              text{ x.path .. " · " .. (x.files or 0) .. " files, " .. kb(x.bytes), size = "xs", color = "text-dim" },
            },
            x.keepable and button{ is_kept and "Replace…" or "Keep…", key = "keep-" .. x.name, on = "keep", arg = { sandbox = look_for, name = x.name },
              help = "Every sandbox created from now on gets it" }
              or text{ "name not kept", size = "sm", color = "text-dim", help = "Lowercase letters, digits, - and _ only" },
          }
        end
        if #rows > 0 then
          local l, more = paged(rows, state.more.found, "found")
          out[#out + 1] = l
          out[#out + 1] = more
        else
          out[#out + 1] = note("No skill in " .. look_for .. ": none under the project's .claude/skills/ or the sandbox's own ~/.claude/skills/.")
        end
      end
    end
    if not wide then
      main[#main + 1] = column(out)
      return column{ h = "full", w = "full", scroll{ gap = 0, column(main) } }
    end
    return split{ id = "skills-split", vertical = true, ratio = 0.46,
      column{ h = "full", w = "full", px = 4, scroll{ gap = 0, column(main) } },
      column{ h = "full", w = "full", px = 4, scroll{ gap = 0, column(out) } } }
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
