-- New sandbox: the chooser, the workspace dialog and the chat dialog
-- (assets/js/create.js). The form is the core's (data.create): this screen
-- shows it and changes it one intent at a time. Creating, and every question
-- about access to this machine, is the core's: what it confirms is what it
-- holds, never text from here.

local function hint(s, color)
  return text{ s, wrap = true, size = "sm", color = color or "text-muted" }
end

local function title(s)
  return text{ s, weight = "semibold", color = "text-bright" }
end

local function head(t)
  return row{ w = "full", gap = 8,
    text{ t, size = "lg", weight = "semibold", color = "text-bright" },
    spacer{},
    button{ icon = "close", variant = "ghost", on = "close", key = "create-close", help = "Close" },
  }
end

-- A two-button switch (RW / RO, No access / Read-only...).
local function switch(key, a, b, on_a, act, arg_a, arg_b, help_a, help_b)
  return row{ gap = 2,
    button{ a, key = key .. "-a", variant = on_a and "primary" or "ghost", on = act, arg = arg_a, help = help_a },
    button{ b, key = key .. "-b", variant = on_a and "ghost" or "primary", on = act, arg = arg_b, help = help_b },
  }
end

-- ---------------------------------------------------------------- chooser

local function card(key, ic, t, lead, points, go)
  local kids = { key = key, on = "open", arg = key == "kind-chat" and "chat" or "workspace",
    grow = 1, gap = 8, pad = 12, radius = "md", border = "border", fill = "surface", align = "start",
    row{ gap = 8, icon{ ic, size = 14, color = "blue" }, title(t) },
    hint(lead, "text"),
  }
  for _, p in ipairs(points) do kids[#kids + 1] = hint("· " .. p) end
  kids[#kids + 1] = text{ go, size = "sm", color = "blue" }
  return column(kids)
end

local function chooser()
  return column{ w = "full", gap = 12,
    head("New sandbox"),
    hint("Both run an agent in an isolated container. What differs is what the agent can see.", "text"),
    row{ w = "full", gap = 12, align = "stretch",
      card("kind-workspace", "folder", "Workspace sandbox", "An agent on one of your project folders.",
        { "Mounts a folder from your machine", "Reads, edits and runs your code", "Port forwarding, SSH and the file browser", "You choose the name and the folder" },
        "Choose folder →"),
      card("kind-chat", "chat", "Chat sandbox", "An agent on a fresh empty folder.",
        { "None of your code is mounted", "Nothing of yours to read or change", "Nothing to configure: the name is optional", "Throwaway: for asking, not for building" },
        "Start chatting →"),
    },
  }
end

-- ---------------------------------------------------------------- chat

local function chat(c)
  return column{ w = "full", gap = 12,
    head("New chat sandbox"),
    hint("An agent on an empty folder: none of your code is mounted.", "text"),
    column{ w = "full", gap = 4,
      title("Name"),
      input{ id = "create-chat-name", value = c.chat_name, placeholder = c.chat_suggestion,
        on = "set", arg = "chat_name", on_submit = "chat" },
      hint("Optional. Left empty, the sandbox is called " .. (c.chat_suggestion or "chat-…") .. "."),
      c.chat_note and hint(c.chat_note, "red") or false,
      c.chat_problem and hint(c.chat_problem, "red") or false,
    },
    c.error and hint(c.error, "red") or false,
    row{ w = "full", gap = 8,
      button{ "← Back", variant = "ghost", on = "open", arg = "chooser", key = "create-back" },
      spacer{},
      button{ "Cancel", on = "close", key = "create-cancel" },
      button{ "Start chat", icon = "chat", variant = "primary", on = "chat", key = "create-chat",
        disabled = c.chat_problem ~= nil },
    },
  }
end

-- ---------------------------------------------------------------- workspace

local function folder_browser(c)
  local l = c.listing
  local picking = c.carve_pick ~= nil
  local rows = {}
  if l then
    if l.can_up then
      rows[#rows + 1] = row{ key = "fs-up", on = "browse", arg = l.parent, w = "full", gap = 8, py = 4, px = 4, radius = "xs",
        icon{ "chevron-right", size = 12, color = "text-muted" }, text{ "..", color = "text-muted" } }
    end
    for _, e in ipairs(l.entries) do
      rows[#rows + 1] = row{ key = "fs-" .. e.name, on = "browse", arg = e.path, w = "full", gap = 8, py = 2, px = 4, radius = "xs",
        icon{ "folder", size = 12, color = "text-muted" },
        text{ e.name, truncate = true, grow = 1 },
        (not picking) and button{ e.added and "✓" or "+", key = "add-" .. e.name, variant = e.added and "primary" or "ghost",
          on = "repo_toggle", arg = e.path, help = e.added and "Remove from this sandbox" or "Add to this sandbox's folders" } or false,
        (not picking and not c.remote) and button{ e.starred and "★" or "☆", key = "star-" .. e.name, variant = "ghost",
          color = e.starred and "amber" or "text-muted", on = "star", arg = e.path,
          help = e.starred and "Unstar this folder" or "Star this folder: pins it as a shortcut above" } or false,
      }
    end
    if #l.entries == 0 then rows[#rows + 1] = hint("No subdirectories") end
  elseif c.listing_error then
    rows[#rows + 1] = hint(c.listing_error, "red")
  else
    rows[#rows + 1] = hint("Loading...")
  end
  rows.h = 170
  rows.gap = 2
  rows.key = "fs-list"

  local favs = false
  if #c.favourites > 0 and not c.remote and not picking then
    local chips = { gap = 4, w = "full" }
    for _, f in ipairs(c.favourites) do
      chips[#chips + 1] = button{ "★ " .. f.name, key = "fav-" .. f.name, on = "browse", arg = f.path,
        color = f.missing and "text-muted" or "amber", help = f.missing and (f.path .. ": not found right now") or f.path }
    end
    favs = row(chips)
  end

  return column{ w = "full", gap = 4,
    row{ w = "full", gap = 8,
      title("Project folder"), spacer{},
      (not c.remote) and button{ "Browse...", icon = "folder", on = "pick", arg = "browse", key = "create-native",
        help = "Your system's folder picker" } or false,
    },
    hint("The agent works in this folder and can change its files."),
    favs,
    row{ w = "full", gap = 8, px = 8, py = 4, radius = "xs", fill = "surface-alt",
      badge{ c.path_tag, tone = c.path_tag == "Selected" and "green" or "neutral" },
      text{ c.path ~= "" and c.path or "-", mono = true, size = "sm", truncate = true, grow = 1, key = "create-path" },
    },
    column{ w = "full", border = "border", radius = "sm", pad = 4, scroll(rows) },
    hint("Click a name to open it.  + extra folder" .. (c.remote and "" or "  ☆ shortcut")),
    c.remote and column{ w = "full", gap = 4,
      hint("Or clone a git URL on the host, into ~/sbxw-repos/:"),
      input{ id = "create-git-url", value = c.git_url, placeholder = "https://github.com/you/repo.git", on = "set", arg = "git_url" },
    } or false,
  }
end

local function option(t, desc, control)
  return row{ w = "full", gap = 12, align = "start",
    column{ grow = 1, gap = 2, title(t), desc and hint(desc) or false },
    control or false,
  }
end

local function repos(c)
  local rows = { w = "full", gap = 4 }
  for i, r in ipairs(c.repos) do
    rows[#rows + 1] = row{ w = "full", gap = 8,
      text{ r.path, mono = true, size = "sm", truncate = true, grow = 1, help = r.path },
      i == 1 and badge{ "workspace", tone = "green", help = "The workspace is always read-write: threads write their worktrees into it" }
        or button{ "⤒ workspace", variant = "ghost", on = "repo_main", arg = i, key = "repo-main-" .. i, help = "Make this the workspace" },
      i > 1 and switch("repo-mode-" .. i, "RW", "RO", not r.readonly, "repo_mode", { i = i, ro = false }, { i = i, ro = true },
        "The agent may change it", "The agent can only read it") or false,
      button{ icon = "close", variant = "ghost", on = "repo_rm", arg = i, key = "repo-rm-" .. i, help = "Remove" },
    }
  end
  return column{ w = "full", gap = 8,
    option("Extra folders", false,
      button{ "+ Add this folder", on = "repo_add", key = "create-add-repo", disabled = not c.can_add_repo }),
    hint(c.repo_hint, c.repo_hint_error and "red" or "text-muted"),
    #c.repos > 0 and column(rows) or false,
  }
end

local function carves(c)
  local kids = { w = "full", gap = 8,
    option("Restricted sub-folders", "Keep a folder inside a read-write folder out of the agent's reach, or read-only."),
  }
  local adds = { gap = 8, w = "full" }
  for i, p in ipairs(c.carve_parents) do
    adds[#adds + 1] = button{ "+ Restrict a sub-folder" .. (#c.carve_parents > 1 and (" of " .. p.label) or ""),
      key = "carve-add-" .. i, on = "carve_pick", arg = p.path, disabled = c.carve_pick ~= nil, help = p.path }
  end
  if #adds > 0 then kids[#kids + 1] = row(adds) end
  local pk = c.carve_pick
  if pk then
    kids[#kids + 1] = column{ w = "full", gap = 4, pad = 8, radius = "sm", fill = "blue-tint",
      hint("Choose a sub-folder of " .. pk.root .. " in the list above (you cannot leave it).", "text"),
      hint("Now in: " .. pk.cur),
      row{ w = "full", gap = 8,
        button{ "Use this folder", on = "carve", arg = "use", key = "carve-use", disabled = not pk.can_use },
        column{ grow = 1, input{ id = "create-carve-typed", value = pk.typed, placeholder = "or type a path inside it",
          on = "set", arg = "carve_typed", on_submit = "carve_typed" } },
        button{ "Add", on = "carve", arg = "typed", key = "carve-typed-add" },
        button{ "Cancel", variant = "ghost", on = "carve", arg = "cancel", key = "carve-cancel" },
      },
      pk.error and hint(pk.error, "red") or false,
    }
  end
  for i, cv in ipairs(c.carves) do
    kids[#kids + 1] = column{ w = "full", gap = 4, pad = 8, radius = "sm", border = cv.error and "red-border" or "border",
      row{ w = "full", gap = 8,
        cv.parent_label and badge{ "in " .. cv.parent_label } or false,
        text{ cv.rel, mono = true, size = "sm", truncate = true, grow = 1, help = cv.path },
        switch("carve-access-" .. i, "No access", "Read-only", cv.access == "none", "carve_set",
          { i = i, v = "none" }, { i = i, v = "ro" }, "The agent cannot see it", "The agent can read it, not change it"),
        switch("carve-enforce-" .. i, "Strict", "Soft", cv.enforce == "strict", "carve_set",
          { i = i, v = "strict" }, { i = i, v = "soft" }, "The folder never enters the sandbox", "The parent is shared whole and the folder is covered"),
        button{ icon = "close", variant = "ghost", on = "carve_set", arg = { i = i, v = "remove" }, key = "carve-rm-" .. i, help = "Remove" },
      },
      hint(cv.note),
      cv.hidden_count > 0 and row{ w = "full", gap = 8,
        icon{ "warning", size = 12, color = "amber" },
        text{ cv.hidden_count .. (cv.hidden_count == 1 and " file" or " files") .. " next to it will not be visible in the sandbox.",
          wrap = true, size = "sm", color = "amber", grow = 1 },
        button{ "Switch to soft", variant = "ghost", on = "carve_set", arg = { i = i, v = "soft" }, key = "carve-soft-" .. i },
      } or false,
      cv.error and hint(cv.error, "red") or false,
    }
  end
  local pv = c.preview
  if pv then
    local lines = { w = "full", gap = 2, pad = 8, radius = "sm", fill = pv.bad and "red-tint" or "surface-alt", key = "carve-preview",
      text{ pv.summary, wrap = true, size = "sm", color = pv.bad and "red" or "text" } }
    for _, w in ipairs(pv.warnings or {}) do lines[#lines + 1] = hint("⚠ " .. w, "amber") end
    for _, f in ipairs(pv.files or {}) do lines[#lines + 1] = text{ f, mono = true, size = "xs", color = "text-muted", truncate = true } end
    if (pv.more or 0) > 0 then lines[#lines + 1] = hint("and " .. pv.more .. " more") end
    if pv.stub then lines[#lines + 1] = hint("The main folder itself is not shared: nothing can be created directly in it.") end
    kids[#kids + 1] = column(lines)
  end
  return column(kids)
end

local function ports(c)
  local kids = { w = "full", gap = 4,
    option("Ports", "Open a server running in the sandbox from your browser.",
      button{ "+ Add port", on = "port_add", key = "create-add-port" }),
  }
  if #c.ports > 0 then
    kids[#kids + 1] = row{ w = "full", gap = 8,
      text{ "In sandbox", size = "xs", color = "text-muted", w = 80 }, text{ "", w = 12 },
      text{ "On your machine", size = "xs", color = "text-muted", w = 100 }, text{ "Hostname", size = "xs", color = "text-muted", grow = 1 } }
  end
  for i, p in ipairs(c.ports) do
    kids[#kids + 1] = row{ w = "full", gap = 8,
      column{ w = 80, input{ id = "port-" .. i .. "-sandbox", value = p.sandbox, placeholder = "3000", on = "set", arg = { f = "port_sandbox", i = i } } },
      text{ "→", w = 12, color = "text-muted" },
      column{ w = 100, input{ id = "port-" .. i .. "-host", value = p.host, placeholder = p.sandbox ~= "" and p.sandbox or "same", on = "set", arg = { f = "port_host", i = i } } },
      column{ grow = 1, input{ id = "port-" .. i .. "-alias", value = p.alias, placeholder = "app.local", on = "set", arg = { f = "port_alias", i = i } } },
      button{ icon = "close", variant = "ghost", on = "port_rm", arg = i, key = "port-rm-" .. i, help = "Remove" },
    }
  end
  return column(kids)
end

local function kit_folders(c)
  if #c.kit_mounts == 0 or c.remote then return false end
  local kids = { w = "full", gap = 4,
    option("Kit folders", "Folders on your machine behind the paths your kits share with it. Left empty, sbx keeps its own folder for that kit."),
  }
  for i, k in ipairs(c.kit_mounts) do
    kids[#kids + 1] = row{ w = "full", gap = 8,
      text{ k.target, mono = true, size = "sm", w = 150, truncate = true, help = "Declared by " .. k.kit },
      column{ grow = 1, input{ id = "kit-" .. i, value = k.host, placeholder = "sbx's own folder", on = "set", arg = { f = "kit_mount", i = i } } },
      button{ "Choose...", on = "pick", arg = i, key = "kit-pick-" .. i },
    }
  end
  return column(kids)
end

local function workspace(c)
  local host = false
  if #c.hosts > 0 then
    host = column{ w = "full", gap = 4,
      title("Host"),
      select{ id = "create-host", value = c.host, items = c.hosts, on = "host" },
      hint("Where the sandbox runs. Remote hosts are experimental."),
    }
  end
  local body = { gap = 16, w = "full", key = "create-scroll", grow = 1,
    host,
    column{ w = "full", gap = 4,
      title("Name"),
      input{ id = "create-name", value = c.name, placeholder = "my-project", on = "set", arg = "name" },
      hint("Lowercase letters, digits and dashes."),
      c.name_note and hint(c.name_note, "red") or false,
      (c.name ~= "" and c.name_problem) and hint(c.name_problem, "red") or false,
    },
    folder_browser(c),
    divider{},
    text{ "Optional", size = "caption", weight = "semibold", color = "text-muted" },
    repos(c),
    carves(c),
    ports(c),
    kit_folders(c),
    option("Remote control", "Use this sandbox from claude.ai/code or the Claude app. Needs `sbx settings set claude.remoteControl true` once (before sbx 0.53: /login in its pane instead).",
      checkbox{ "", value = c.remote_control, on = "toggle", arg = "remote_control", key = "create-rc" }),
    option("Claude in Chrome", "The agent opens and reads tabs in your browser. Needs the Claude extension, a subscription login (an API key alone is refused) and `sbx settings set claude.chrome true` once.",
      checkbox{ "", value = c.chrome, on = "toggle", arg = "chrome", key = "create-chrome" }),
    (c.template and not c.remote) and option("Rebuild the starting template", c.template.note,
      checkbox{ "", value = c.rebuild, on = "toggle", arg = "rebuild", key = "create-rebuild", help = c.template.help }) or false,
  }
  return column{ w = "full", h = "full", gap = 12,
    head("New sandbox"),
    scroll(body),
    c.error and hint(c.error, "red") or false,
    row{ w = "full", gap = 8,
      button{ "← Back", variant = "ghost", on = "open", arg = "chooser", key = "create-back" },
      spacer{},
      (c.create_problem and c.name ~= "") and text{ c.create_problem, size = "sm", color = "text-muted", truncate = true, max_w = 300 } or false,
      button{ "Cancel", on = "close", key = "create-cancel" },
      button{ "Create", icon = "check", variant = "primary", on = "create", key = "create-submit", disabled = not c.can_create },
    },
  }
end

return {
  render = function(state, data)
    local c = data.create or {}
    local d = c.dialog or (data.app.modal and data.app.modal.arg) or "chooser"
    if d == "workspace" and c.name ~= nil then return workspace(c) end
    if d == "chat" and c.chat_suggestion ~= nil then return chat(c) end
    return chooser()
  end,

  actions = {
    open = function(_, d) return intent("create_open", { dialog = d }) end,
    close = function() return intent("close_modal") end,
    host = function(_, _, h) return intent("create_host", { host = h }) end,
    browse = function(_, path) return intent("create_browse", { path = path }) end,
    pick = function(_, which)
      if which == "browse" then return intent("create_pick", {}) end
      return intent("create_pick", { kit = which })
    end,
    star = function(_, path) return intent("create_star", { path = path }) end,
    repo_toggle = function(_, path) return intent("create_repo", { action = "toggle", path = path }) end,
    repo_add = function() return intent("create_repo", { action = "add" }) end,
    repo_main = function(_, i) return intent("create_repo", { action = "main", index = i }) end,
    repo_mode = function(_, a) return intent("create_repo", { action = a.ro and "ro" or "rw", index = a.i }) end,
    repo_rm = function(_, i) return intent("create_repo", { action = "remove", index = i }) end,
    carve_pick = function(_, root) return intent("create_carve", { action = "pick", path = root }) end,
    carve = function(_, what) return intent("create_carve", { action = what }) end,
    carve_typed = function() return intent("create_carve", { action = "typed" }) end,
    carve_set = function(_, a) return intent("create_carve", { action = a.v, index = a.i }) end,
    port_add = function() return intent("create_port", { action = "add" }) end,
    port_rm = function(_, i) return intent("create_port", { action = "remove", index = i }) end,
    set = function(_, a, v)
      local args = { field = type(a) == "table" and a.f or a, index = type(a) == "table" and a.i or nil }
      if v ~= nil and v ~= "" then args.value = v end
      return intent("create_set", args)
    end,
    toggle = function(_, field, on) return intent("create_toggle", { field = field, on = on }) end,
    create = function() return intent("create_submit") end,
    chat = function() return intent("create_chat") end,
  },
}
