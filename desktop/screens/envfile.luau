-- A sandbox's environment file, sbxenv.yaml (the web UI's envfile.js card):
-- previewed as it would be written, with the one choice that matters, the
-- folder it goes in. data.app.modal.arg is the sandbox; data.envfile is the
-- daemon's last answer: { sandbox, yaml, dest, dir, asked, written, error, loading }.

local current = nil
local yaml_now = ""

return {
  init = function() return { dir = nil, for_ = nil, overwrite = false } end,

  render = function(state, data)
    local name = (data.app.modal or {}).arg
    current = name
    local e = data.envfile or {}
    yaml_now = e.yaml or ""
    if state.for_ ~= name then
      state.for_ = name
      state.dir = nil
      state.overwrite = false
    end
    local dir = state.dir or e.dir or ""
    local note
    if e.asked and e.asked ~= "" and e.dir and e.asked ~= e.dir then
      note = e.asked .. " does not contain the workspace, so the file goes in its parent instead."
    else
      note = "Paths are written relative to this folder. Any folder above the workspace will do."
    end
    return column{ gap = 12, w = "full", h = "full",
      row{ w = "full", gap = 8,
        icon{ "file", size = 14, color = "text-muted" },
        text{ "Environment file: " .. tostring(name), size = "lg", weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
        button{ icon = "close", variant = "ghost", on = "close", key = "env-close", help = "Close" },
      },
      text{ "sbxenv.yaml describes this sandbox so `sbx` can create it again from the file, on any machine with the same folders below it.", size = "sm", color = "text-muted", wrap = true },
      row{ gap = 8, w = "full",
        text{ "Folder", size = "sm", color = "text-muted" },
        column{ grow = 1, min_w = 0, input{ id = "env-dir", value = dir, placeholder = "a folder above the workspace", on = "dir", on_submit = "preview" } },
        button{ "Preview", icon = "reload", on = "preview", key = "env-preview" },
      },
      text{ note, size = "xs", color = "text-dim", wrap = true },
      e.error and row{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "red-tint", border = "red",
        icon{ "warning", size = 12, color = "red" },
        text{ e.error, size = "sm", color = "text-bright", wrap = true, grow = 1 },
      } or false,
      column{ w = "full", grow = 1, min_h = 80, pad = 8, radius = "sm", fill = "bg", border = "border",
        scroll{ text{ e.loading and "Loading…" or (e.yaml or ""), mono = true, size = "sm", color = "text", wrap = true } },
      },
      row{ gap = 8, w = "full",
        text{ e.dest and ("Writes " .. e.dest) or "", size = "sm", color = "text-muted", truncate = true, grow = 1, help = e.dest },
      },
      row{ gap = 8, w = "full",
        checkbox{ "Overwrite an existing file", value = state.overwrite, on = "overwrite", key = "env-overwrite" },
        spacer{},
        button{ "Copy", icon = "copy", on = "copy", disabled = (e.yaml or "") == "", key = "env-copy" },
        button{ "Save file", icon = "check", variant = "primary", on = "save", busy = e.loading == true, key = "env-save" },
      },
    }
  end,

  actions = {
    close = function() return intent("close_modal") end,
    dir = function(state, _, t) state.dir = t or "" end,
    preview = function(state) return intent("envfile_preview", { sandbox = current, dir = state.dir or "" }) end,
    overwrite = function(state, _, v) state.overwrite = v and true or false end,
    save = function(state)
      local o = state.overwrite
      state.overwrite = false
      return intent("envfile_save", { sandbox = current, dir = state.dir or "", overwrite = o })
    end,
    copy = function()
      if yaml_now == "" then return nil end
      return intent("copy_text", { text = yaml_now })
    end,
  },
}
