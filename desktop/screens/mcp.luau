-- MCP servers (an overlay over the pane area, opened from the rail; it was
-- Setup's MCP tab): the servers registered on this machine (sbx mcp),
-- loading one into a running sandbox, signing in to a remote one, adding
-- one. The web UI's mcp.js. One column at every width: the form on top
-- (Name and Kind on one row when there is room), the registered servers
-- under it, each a full-width row with its actions; pressing a row opens
-- its details (sbx mcp inspect) under it.
--
-- The sandbox Load and Unload act on is data.mcp_view.sandbox (the panel
-- opened for a sandbox), else the last one used (data.app.kept.mcp.sandbox),
-- else the first running one. Picking another asks `mcp_pick`.
--
-- Secrets: a header's ${KEY} is named here, never valued. Its value is typed
-- in the core's dialog after Add; this screen never sees it. Unload, Remove
-- and Add are confirmed in the core's dialog, not here.

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

local function is_stdio(x)
  local t = string.lower(x.type or "")
  return string.find(t, "stdio") ~= nil or string.find(t, "local") ~= nil or string.find(t, "command") ~= nil
    or (type(x.detail) == "table" and x.detail.command ~= nil)
end

local function placeholders(headers)
  local keys = {}
  for k in string.gmatch(headers or "", "%${([%w_%-]+)}") do
    if not contains(keys, k) then keys[#keys + 1] = k end
  end
  return keys
end

return {
  init = function() return { mcp = nil, more = 0 } end,

  render = function(state, data)
    local errors = (data.app and data.app.errors) or {}
    local m = data.mcp or {}
    local err = errors.mcp or failed(data.mcp)
    local servers = {}
    for _, x in ipairs(m.servers or {}) do servers[#servers + 1] = { name = x.name, type = x.type, stdio = is_stdio(x) } end

    -- The sandbox Load and Unload act on.
    local running, names = {}, {}
    for _, sb in ipairs(data.sandboxes or {}) do
      if sb.status == "running" then running[#running + 1] = { id = sb.name, label = sb.name }; names[#names + 1] = sb.name end
    end
    local asked = (data.mcp_view or {}).sandbox
    local want = asked or ((((data.app or {}).kept or {}).mcp or {}).sandbox)
    local target = contains(names, want) and want or names[1]
    local wide = overlay_wide(data)
    local sel = data.mcp_inspect_for

    -- One column: a quiet line, the form, the servers under it.
    local out = { w = "full", gap = 12, key = "mcp-body",
      -- Beside the overlay's title (the core's header draws it: TITLE_SLOT).
      -- The scope (scope.rs): the running sandbox servers load into.
      row{ key = "title-slot", gap = 8, select{ id = "mcp-sandbox", value = target or "", on = "pick", scope = { kind = "sandbox", running = true },
        placeholder = "No running sandbox", disabled = #running == 0 } },
      row{ w = "full", gap = 4, key = "mcp-about",
        help = "An MCP server gives an agent more tools (an issue tracker, a browser, a database). It is registered once on this machine, then loaded into each running sandbox that needs it.",
        icon{ "info", size = 12, color = "text-muted" },
        text{ "More tools for an agent: registered once on this machine, loaded into a running sandbox.", size = "sm", color = "text-muted", truncate = true, grow = 1 } },
    }
    if err then
      out[#out + 1] = row{ w = "full", gap = 8, key = "mcp-error",
        icon{ "warning", size = 12, color = "red" },
        text{ err, wrap = true, size = "sm", color = "red", grow = 1 },
        button{ "Read again", icon = "reload", variant = "ghost", on = "reload", key = "mcp-reload" },
      }
    end
    if asked and asked ~= target then
      -- Opened for a stopped sandbox: the picker is on the first running one,
      -- and this line says so with the same name the picker and Load show.
      out[#out + 1] = note("Opened for " .. asked .. ", which is not running: Load acts on " .. (target or "no sandbox") .. " (the picker). Start " .. asked .. ", or pick another.", "amber")
    elseif #running == 0 then
      out[#out + 1] = note("No sandbox is running: start one to load a server into it.")
    end

    -- Each running sandbox's MCP mode and the servers connected now (sbx
    -- 0.54, data.mcp_mode by sandbox name). An older sbx, a remote sandbox
    -- or a daemon without the route: "needs sbx 0.54" or the daemon's note,
    -- muted, never an error. Server names are sbx's: set as text.
    local modes = data.mcp_mode
    local mode_errs = type(errors.mcp_mode) == "table" and errors.mcp_mode or {}
    local connected = {}
    if type(modes) == "table" and #running > 0 then
      local rows_ = {}
      for _, sbn in ipairs(names) do
        local m = modes[sbn]
        local why = mode_errs[sbn]
        local word, tone, line, help
        if type(m) == "table" and m.supported ~= false and m.ok ~= false then
          word = m.mode or "unknown"
          tone = word == "dynamic" and "blue" or "neutral"
          local list_ = m.servers or {}
          line = #list_ > 0 and table.concat(list_, ", ") or "no server connected"
          help = word == "dynamic" and "Dynamic: the agent connects servers as it needs them; this is what is connected now"
            or (word == "static" and "Static: the servers loaded at start stay connected" or "sbx says something sbxw does not read")
          if sbn == target then for _, x in ipairs(list_) do connected[x] = true end end
        elseif type(m) == "table" then
          word, tone, line = "unknown", "neutral", m.note or "needs sbx 0.54"
        elseif why then
          word, tone, line, help = "unknown", "neutral", "needs sbx 0.54", why
        else
          word, tone, line = nil, nil, "Reading…"
        end
        rows_[#rows_ + 1] = row{ w = "full", gap = 8, key = "mcp-mode-" .. sbn, help = help,
          text{ sbn, mono = true, color = sbn == target and "text-bright" or "text", truncate = true, w = 140 },
          word and badge{ word, tone = tone, key = "mcp-mode-word-" .. sbn } or false,
          text{ line, size = "sm", color = "text-muted", truncate = true, grow = 1, min_w = 0, help = line },
        }
      end
      out[#out + 1] = column{ w = "full", gap = 4, key = "mcp-modes",
        row{ w = "full", gap = 8,
          text{ "Connected now", size = "caption", weight = "semibold", color = "text-muted" },
          text{ "MCP mode of each running sandbox, as sbx inspect says", size = "xs", color = "text-muted", truncate = true, grow = 1, min_w = 0 } },
        list(rows_) }
    end

    -- The form, on top. Added: the server is listed now, so the form starts
    -- again (as the web UI resets it). The next edit drops the old fields (`reset`).
    local f = state.mcp or {}
    local done = false
    if f.sent and not f.replace then
      for _, x in ipairs(servers) do if x.name == f.sent then done = true end end
    end
    if done then f = {} end
    local stdio = f.kind == "stdio"
    local keys = placeholders(f.headers)
    -- Two fields on a row when there is room (each keeps 240 pt), else one under the other.
    local function pair(a, b) return row{ w = "full", gap = 12, wrap = true, a, b or false } end
    local function cell(label, child, w)
      return row{ gap = 8, grow = w and nil or 1, w = w, min_w = w and nil or 240, text{ label, size = "sm", color = "text-muted", w = 76 }, child }
    end
    local form = { w = "full", gap = 8, key = "mcp-form", px = 12, py = 12, radius = "md", fill = "surface", border = "elevated",
      text{ f.replace and ("Edit " .. f.replace) or "Add a server", size = "sm", weight = "semibold", color = "text-bright" },
      pair(cell("Name", input{ id = "mcp-name", value = f.name or "", on = "field", arg = { k = "name", reset = done }, placeholder = "github" }),
        cell("Kind", select{ id = "mcp-kind", value = stdio and "stdio" or "remote", on = "kind", arg = { reset = done }, items = {
          { id = "remote", label = "Remote (an HTTP endpoint)" },
          { id = "stdio", label = "Local command (runs on this machine)" } } }, wide and 380 or nil)),
    }
    if stdio then
      form[#form + 1] = pair(cell("Command", input{ id = "mcp-command", value = f.command or "", on = "field", arg = { k = "command", reset = done }, placeholder = "npx" }),
        cell("Arguments", input{ id = "mcp-args", value = f.args or "", on = "field", arg = { k = "args", reset = done }, placeholder = "-y @modelcontextprotocol/server-filesystem /srv" }))
      form[#form + 1] = cell("Directory", input{ id = "mcp-dir", value = f.dir or "", on = "field", arg = { k = "dir", reset = done }, placeholder = "optional" })
    else
      form[#form + 1] = pair(cell("URL", input{ id = "mcp-url", value = f.url or "", on = "field", arg = { k = "url", reset = done }, placeholder = "https://mcp.example.com/mcp" }),
        cell("Headers", input{ id = "mcp-headers", value = f.headers or "", on = "field", arg = { k = "headers", reset = done },
          placeholder = "Authorization: Bearer ${TOKEN}; one per ;" }))
    end
    local hint
    if stdio then
      hint = note("A local command runs on this machine, outside any sandbox: the next dialog shows the exact line and asks you to confirm you read it.", "amber")
    elseif #keys > 0 then
      hint = note("Secrets asked in the next dialog: " .. table.concat(keys, ", ") .. (f.replace and " (left empty: unchanged)" or ""))
    else
      hint = note("OAuth servers are added unauthorized: use Sign in once added.")
    end
    form[#form + 1] = row{ w = "full", gap = 8, wrap = true,
      column{ grow = 1, min_w = 200, hint },
      (f.replace or trim(f.name) ~= "") and button{ "Clear", variant = "ghost", on = "clear", key = "mcp-clear" } or false,
      button{ f.replace and "Save…" or "Add…", key = "mcp-save", variant = "primary", on = "save", disabled = trim(f.name) == "" },
    }
    out[#out + 1] = column(form)

    -- The servers, under the form, the whole width: the first PAGE (+ "Show more").
    out[#out + 1] = row{ w = "full", gap = 8, wrap = true, key = "mcp-list-head",
      text{ "Registered on this machine", size = "caption", weight = "semibold", color = "text-muted" }, badge{ tostring(#servers) },
      text{ "Load and Unload are both offered; \"connected\" is what sbx says is connected now.", size = "xs", color = "text-muted", truncate = true, grow = 1, min_w = 0,
        help = "Load and Unload are both offered. A server marked connected is one sbx 0.54 reports connected in the sandbox picked; the list keeps the order of this machine's registrations." } }
    local shown = PAGE + (state.more or 0)
    local rows = {}
    for i, x in ipairs(servers) do
      if i > shown then break end
      local open = sel == x.name
      local acts = row{ gap = 4, wrap = true,
        button{ "Load", key = "mcp-load-" .. x.name, variant = "ghost", on = "load", arg = { server = x.name, sandbox = target }, disabled = not target,
          help = "Attach to " .. (target or "a running sandbox") .. ": its agent picks up the tools without a restart" },
        button{ "Unload", key = "mcp-unload-" .. x.name, variant = "ghost", on = "unload", arg = { server = x.name, sandbox = target }, disabled = not target },
        (not x.stdio) and button{ "Sign in", key = "mcp-auth-" .. x.name, variant = "ghost", on = "auth", arg = x.name,
          help = "sbx mcp auth: sign in from this machine's browser; the token serves every sandbox" } or false,
        button{ "Inspect", key = "mcp-inspect-" .. x.name, variant = "ghost", on = open and "inspect_close" or "inspect", arg = x.name, help = open and "Hide the details" or nil },
        button{ icon = "trash", variant = "ghost", color = "red", on = "rm", arg = x.name, key = "mcp-rm-" .. x.name, help = "Unregister from this machine" },
      }
      local kids = { w = "full", gap = 4, key = "mcp-" .. x.name, px = 8, py = 4, radius = "sm", fill = open and "surface" or nil, border = open and "elevated" or nil,
        row{ w = "full", gap = 8, wrap = not wide,
          row{ gap = 8, grow = 1, min_w = 200, px = 4, h = 28, radius = "xs", key = "mcp-sel-" .. x.name, on = open and "inspect_close" or "inspect", arg = x.name,
            fill = open and "active-bg" or nil, hover = "hover-subtle", help = open and ("Hide " .. x.name .. "'s details") or ("Show " .. x.name .. "'s details"),
            icon{ open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
            dot{ tone = x.stdio and "amber" or "blue" },
            text{ x.name, mono = true, truncate = true, color = "text-bright" },
            badge{ x.stdio and "local command" or "remote", tone = x.stdio and "amber" or "blue",
              help = x.stdio and "Runs on this machine, outside any sandbox, each time a sandbox loads it" or "An HTTP endpoint" },
            connected[x.name] and badge{ "connected in " .. target, tone = "green", key = "mcp-on-" .. x.name } or false,
            spacer{} },
          acts },
      }
      if open then
        local t = data.mcp_inspect
        local body = type(t) == "table" and (t.text or t.error or "") or "Reading…"
        kids[#kids + 1] = column{ w = "full", gap = 8, px = 24, py = 4, key = "mcp-inspect",
          note(x.stdio and "Runs on this machine, outside any sandbox, each time a sandbox loads it. Its tools reach the agent of the sandbox it is loaded into."
            or "An HTTP endpoint the sandbox's agent calls through sbx. Sign in once on this machine: the token serves every sandbox."),
          row{ w = "full", gap = 4, wrap = true,
            button{ "Load into " .. (target or "a sandbox"), key = "mcp-detail-load", icon = "plus", variant = "neutral", on = "load", arg = { server = sel, sandbox = target }, disabled = not target },
            button{ "Unload", key = "mcp-detail-unload", variant = "ghost", on = "unload", arg = { server = sel, sandbox = target }, disabled = not target },
            (not x.stdio) and button{ "Sign in", key = "mcp-detail-auth", variant = "ghost", on = "auth", arg = sel } or false,
            spacer{},
            button{ icon = "close", variant = "ghost", on = "inspect_close", key = "mcp-inspect-close", help = "Hide the details" } },
          text{ "sbx mcp inspect " .. sel, size = "caption", weight = "semibold", color = "text-muted" },
          text{ body ~= "" and body or "(no output)", mono = true, wrap = true, size = "sm" },
        }
      end
      rows[#rows + 1] = column(kids)
    end
    if #rows > 0 then
      out[#out + 1] = list(rows)
      if #servers > shown then
        local n = math.min(PAGE, #servers - shown)
        out[#out + 1] = row{ w = "full", note("Showing " .. shown .. " of " .. #servers .. "."), spacer{},
          button{ "Show " .. n .. " more", variant = "ghost", on = "more", key = "mcp-more" } }
      end
    elseif not err then
      out[#out + 1] = note("No server registered yet. Add one above: a remote server's URL, or a command that runs on this machine.")
    end
    return column{ h = "full", w = "full", scroll{ gap = 0, column{ w = "full", px = 4, column(out) } } }
  end,

  actions = {
    reload = function() return intent("mcp_show") end,
    pick = function(_, _, v) return intent("mcp_pick", { sandbox = v }) end,
    more = function(state) state.more = (state.more or 0) + PAGE end,
    load = function(_, a) return intent("mcp_load", { server = a.server, sandbox = a.sandbox }) end,
    unload = function(_, a) return intent("mcp_unload", { server = a.server, sandbox = a.sandbox }) end,
    auth = function(_, server) return intent("mcp_auth", { server = server }) end,
    inspect = function(_, server) return intent("mcp_inspect", { server = server }) end,
    inspect_close = function() return intent("mcp_inspect_close") end,
    rm = function(_, server) return intent("mcp_rm", { server = server }) end,
    field = function(state, a, v)
      if a.reset or not state.mcp then state.mcp = {} end
      state.mcp[a.k] = v
    end,
    kind = function(state, a, v)
      if a.reset or not state.mcp then state.mcp = {} end
      state.mcp.kind = v
    end,
    clear = function(state) state.mcp = {} end,
    save = function(state)
      local f = state.mcp or {}
      local args = { name = trim(f.name), kind = f.kind == "stdio" and "stdio" or "remote" }
      if args.kind == "stdio" then
        args.command = trim(f.command)
        if trim(f.args) ~= "" then args.args = trim(f.args) end
        if trim(f.dir) ~= "" then args.dir = trim(f.dir) end
      else
        args.url = trim(f.url)
        if trim(f.headers) ~= "" then args.headers = trim(f.headers) end
      end
      if f.replace then args.replace = f.replace end
      f.sent = args.name
      state.mcp = f
      return intent("mcp_save", args)
    end,
  },
}
