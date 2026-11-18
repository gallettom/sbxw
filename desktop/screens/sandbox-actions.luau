-- A sandbox's actions (the web UI's row buttons and its remove and
-- duplicate dialogs), as a dialog: data.app.modal.arg is the sandbox.
-- Remove and recreate are confirmed again in the core's own dialog.

local function suggest(name)
  local base, n = string.match(name, "^(.*)%-copy%-(%d+)$")
  if base then return base .. "-copy-" .. (tonumber(n) + 1) end
  if string.match(name, "%-copy$") then return name .. "-2" end
  return name .. "-copy"
end

local current = nil

return {
  init = function() return { dup = nil, for_ = nil } end,
  render = function(state, data)
    local name = (data.app.modal or {}).arg
    current = name
    local sb
    for _, s in ipairs(data.sandboxes or {}) do if s.name == name then sb = s end end
    if not sb then
      return column{ gap = 8, text{ "No sandbox named " .. tostring(name) .. ".", color = "text-muted" },
        button{ "Close", on = "close", key = "sa-close" } }
    end
    local dup = (state.for_ == name and state.dup) or suggest(name)
    local running = sb.status == "running"
    return column{ gap = 12, w = "full", h = "full",
      row{ w = "full", gap = 8,
        text{ name, size = "lg", weight = "semibold", color = "text-bright" },
        badge{ sb.status, tone = running and "green" or "neutral" },
        spacer{},
        button{ icon = "close", variant = "ghost", on = "close", key = "sa-close", help = "Close" },
      },
      text{ (sb.workspace and ("Workspace: " .. sb.workspace)) or (sb.chat and "A chat sandbox" or "No workspace folder"), size = "sm", color = "text-muted", wrap = true },
      column{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
        text{ "Duplicate", weight = "semibold", color = "text-bright" },
        text{ "A new sandbox on the same workspace.", size = "sm", color = "text-muted" },
        row{ gap = 8, w = "full",
          input{ id = "sa-dup-name", value = dup, placeholder = "new-name", on = "dup", on_submit = "duplicate" },
          button{ "Duplicate", icon = "copy", on = "duplicate", key = "sa-duplicate" },
        },
      },
      row{ gap = 8, w = "full",
        button{ "Network", icon = "ports", on = "network", disabled = not running, key = "sa-network" },
        button{ "Close its terminals", icon = "terminal", on = "detach", key = "sa-detach" },
      },
      row{ gap = 8, w = "full",
        button{ "MCP servers", icon = "plug", on = "mcp", key = "sa-mcp", help = "The MCP servers panel, on " .. name .. ": load or unload a server" },
        button{ "Skills", icon = "book", on = "skills", key = "sa-skills", help = "The Skills panel, on " .. name .. ": keep a skill it wrote" },
        button{ "Kits", icon = "box", on = "kits", key = "sa-kits", help = "The Kits panel, on the sbxw.toml " .. name .. " uses" },
      },
      row{ gap = 8, w = "full",
        button{ "Save as a template…", icon = "copy", on = "template", key = "sa-template" },
        (sb.workspace and sb.workspace ~= "") and button{ "Environment file…", icon = "file", on = "envfile", key = "sa-envfile" } or false,
      },
      divider{},
      row{ gap = 8, w = "full",
        sb.old_sbx and button{ "Recreate…", icon = "reload", on = "recreate", key = "sa-recreate" } or false,
        spacer{},
        button{ "Remove…", icon = "trash", variant = "danger", on = "remove", key = "sa-remove" },
      },
    }
  end,
  actions = {
    close = function() return intent("close_modal") end,
    dup = function(state, _, t) state.dup = t; state.for_ = current end,
    duplicate = function(state)
      local n = (state.for_ == current and state.dup) or suggest(current)
      return intent("sandbox_duplicate", { sandbox = current, new_name = n })
    end,
    network = function() return { intent("close_modal"), intent("network_show", { sandbox = current }) } end,
    mcp = function() return { intent("close_modal"), intent("mcp_show", { sandbox = current }) } end,
    skills = function() return { intent("close_modal"), intent("skills_show", { sandbox = current }) } end,
    kits = function() return { intent("close_modal"), intent("kits_show", { sandbox = current }) } end,
    detach = function() return intent("sandbox_detach", { sandbox = current }) end,
    template = function() return intent("templates_show", { sandbox = current }) end,
    envfile = function() return intent("envfile_show", { sandbox = current }) end,
    recreate = function() return intent("sandbox_recreate", { sandbox = current }) end,
    remove = function() return intent("sandbox_remove", { sandbox = current }) end,
  },
}
