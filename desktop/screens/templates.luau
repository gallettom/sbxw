-- Templates (the web UI's templates.js): what `sbx template ls` holds, disk
-- usage (`sbx df`), and saving a sandbox as a template. data.app.modal.arg:
-- the sandbox to preselect. Removing is confirmed in the core's own dialog.

-- The sandbox the form shows (render knows the list; actions do not get data).
local chosen_now = nil

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function list(data)
  local t = data.templates or {}
  local err = (data.app.errors or {}).templates
  local out = { gap = 4, w = "full" }
  if err or t.ok == false then
    table.insert(out, note("Could not list templates: " .. tostring(t.error or err), "red"))
    return column(out)
  end
  if t.templates == nil then
    table.insert(out, note("Loading…"))
    return column(out)
  end
  if #t.templates == 0 then
    table.insert(out, note("No template saved yet."))
  end
  for _, tp in ipairs(t.templates) do
    local meta = {}
    if tp.size and tp.size ~= "" then table.insert(meta, tp.size) end
    if tp.created and tp.created ~= "" then table.insert(meta, tp.created) end
    table.insert(out, row{ gap = 8, w = "full", px = 8, py = 4, radius = "sm", fill = "surface-alt", key = "tpl-" .. tp.tag,
      text{ tp.tag, mono = true, color = "text-bright", truncate = true },
      tp.configured and badge{ "in use", tone = "green", help = "sbxw.toml's template: new sandboxes start from it" } or false,
      tp.auto and badge{ "automatic", tone = "blue", help = "Kept by sbxw for template = \"auto\"" } or false,
      text{ table.concat(meta, " · "), size = "sm", color = "text-dim", truncate = true, grow = 1 },
      button{ "Remove", icon = "trash", variant = "ghost", on = "remove", arg = tp.tag, key = "tpl-rm-" .. tp.tag, help = "Remove this template" },
    })
  end
  return column(out)
end

return {
  init = function() return { sandbox = nil, tag = "", verbose = false } end,

  render = function(state, data)
    local arg = (data.app.modal or {}).arg
    local items = {}
    for _, sb in ipairs(data.sandboxes or {}) do table.insert(items, { id = sb.name, label = sb.name }) end
    local chosen = state.sandbox or arg or (items[1] and items[1].id)
    chosen_now = chosen
    local saving = data.templates_saving
    local du = data.disk_usage or {}
    local du_err = (data.app.errors or {}).disk_usage
    local du_text
    if du_err or du.ok == false then
      local e = tostring(du.error or du_err)
      du_text = (string.find(e, "unknown command") or string.find(e, "unknown flag")) and "Needs sbx 0.53 or newer." or ("Could not read disk usage: " .. e)
    else
      du_text = du.text or "Loading…"
    end
    return column{ gap = 12, w = "full", h = "full",
      row{ w = "full", gap = 8,
        icon{ "copy", size = 14, color = "text-muted" },
        text{ "Templates", size = "lg", weight = "semibold", color = "text-bright" },
        spacer{},
        button{ icon = "close", variant = "ghost", on = "close", key = "tpl-close", help = "Close" },
      },
      scroll{ gap = 12,
        note("A template is a saved sandbox: new sandboxes can start from it (sbxw.toml's `template`). Saving takes the sandbox's secrets out first and can take minutes."),
        list(data),
        column{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
          text{ "Save a sandbox as a template", weight = "semibold", color = "text-bright" },
          row{ gap = 8, w = "full",
            select{ id = "tpl-sandbox", items = items, value = chosen, on = "pick", placeholder = "No sandbox" },
            column{ grow = 1, min_w = 0, input{ id = "tpl-tag", value = state.tag, placeholder = "tag, e.g. web-base", on = "tag", on_submit = "save" } },
            button{ saving and "Saving…" or "Save", icon = "check", variant = "primary", on = "save", busy = saving ~= nil, disabled = saving == nil and chosen == nil, key = "tpl-save" },
          },
          saving and note("Saving " .. saving.sandbox .. " as " .. saving.tag .. ": this can take minutes.") or false,
        },
        column{ gap = 8, w = "full",
          row{ gap = 8, w = "full",
            text{ "Disk usage", weight = "semibold", color = "text-bright" },
            spacer{},
            checkbox{ "Every item", value = state.verbose, on = "verbose", key = "tpl-verbose" },
            button{ icon = "reload", variant = "ghost", on = "df", key = "tpl-df", help = "Read again" },
          },
          column{ w = "full", pad = 8, radius = "sm", fill = "bg", border = "border",
            text{ du_text, mono = true, size = "sm", color = "text", wrap = true },
          },
        },
      },
    }
  end,

  actions = {
    close = function() return intent("close_modal") end,
    pick = function(state, _, id) state.sandbox = id end,
    tag = function(state, _, t) state.tag = t or "" end,
    save = function(state, _, _)
      local tag = state.tag
      local sb = state.sandbox or chosen_now
      if sb == nil then return nil end
      state.tag = ""
      return intent("template_save", { sandbox = sb, tag = tag })
    end,
    remove = function(_, tag) return intent("template_rm", { tag = tag }) end,
    verbose = function(state, _, v)
      state.verbose = v and true or false
      return intent("disk_usage", { verbose = state.verbose })
    end,
    df = function(state) return intent("disk_usage", { verbose = state.verbose }) end,
  },
}
