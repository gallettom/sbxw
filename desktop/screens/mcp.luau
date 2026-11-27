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
-- Headers: one row each, a name picked (or Custom) and a value; sent as
-- "Name: value" lines. A value holding ";" is refused in the form.
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

-- Header rows (a Remote server): one name picked from HEADERS (or Custom,
-- typed in the row) and one value each. The screen still sends `headers` as
-- one string: the rows as "Name: value" lines, as the intent takes them.
local HEADERS = { "Authorization", "X-API-Key", "Api-Key", "Cookie" }
local CUSTOM = "custom"
local HEADER_ITEMS = {}
for _, h in ipairs(HEADERS) do HEADER_ITEMS[#HEADER_ITEMS + 1] = { id = h, label = h } end
HEADER_ITEMS[#HEADER_ITEMS + 1] = { id = CUSTOM, label = "Custom…" }

local function value_hint(name)
  if name == "Authorization" then return "Bearer ${TOKEN}" end
  if name == "X-API-Key" or name == "Api-Key" then return "${API_KEY}" end
  return "value, or ${SECRET}"
end

-- A row's header name: the picked one, or the one typed for Custom.
local function row_name(r)
  if r.pick == CUSTOM then return trim(r.name) end
  return r.pick or HEADERS[1]
end

-- Why a row cannot be sent as typed, or nil. An empty name or value is not
-- a reason: that row is skipped. The intent splits headers on ";" and new
-- lines, so a ";" in a value would send something else than what was typed.
local function row_problem(r, seen)
  local n, v = row_name(r), trim(r.value)
  -- A row Edit filled from a listed header whose value sbx did not show
  -- (masked or empty): skipping it would add the server again without that
  -- header, unsaid. Removing the row is the explicit way to drop it.
  if r.listed and n ~= "" and v == "" then return "sbx did not show this header's value: type it again, or remove the row" end
  if n == "" or v == "" then return nil end
  if string.find(n, "[^%w!#%$%%&'%*%+%-%.%^_`|~]") then return "A header name is letters, digits and - _ . only (no space, \":\" or \";\")" end
  if string.find(v, ";", 1, true) then return "A value cannot hold \";\": sbxw sends headers separated by \";\". Put it in a secret, ${NAME}, instead" end
  if seen[string.lower(n)] then return n .. " is already set above" end
  return nil
end

-- The rows as the intent's `headers`: "Name: value" lines, rows with an empty
-- name or value left out; and the first problem, if any.
local function header_lines(rows)
  local lines, seen, bad = {}, {}, nil
  for _, r in ipairs(rows or {}) do
    local why = row_problem(r, seen)
    local n, v = row_name(r), trim(r.value)
    if why then bad = bad or why
    elseif n ~= "" and v ~= "" then lines[#lines + 1] = n .. ": " .. v end
    if n ~= "" and v ~= "" then seen[string.lower(n)] = true end
  end
  return table.concat(lines, "\n"), bad
end

-- A listed server's field, by the first of these paths that holds a string
-- or a list (sbx documents only name and type: the rest is read as the web
-- UI's mcpField reads it, and is data, never markup).
local function field_of(detail, paths)
  for _, path in ipairs(paths) do
    local v = detail
    for part in string.gmatch(path, "[^%.]+") do v = type(v) == "table" and v[part] or nil end
    if type(v) == "string" and v ~= "" then return v end
    if type(v) == "table" and next(v) ~= nil then return v end
  end
  return nil
end

-- A listed value that is only a mask ("****", "•••") is not the value:
-- left empty, never sent back as typed.
local function unmasked(v)
  v = trim(v)
  if v == "" or string.find(v, "^[%*•]+$") then return "" end
  return v
end

-- One listed header ("Name: value", split at the first colon) as a row: a
-- name out of HEADERS is picked, any other is a Custom row.
local function row_of(h, id)
  local n, v = string.match(h, "^([^:]*):(.*)$")
  n, v = trim(n or h), unmasked(v or "")
  for _, known in ipairs(HEADERS) do
    if string.lower(known) == string.lower(n) then return { id = id, pick = known, name = "", value = v, listed = true } end
  end
  return { id = id, pick = CUSTOM, name = n, value = v, listed = true }
end

-- What Edit puts in the form, from a listed server (the row's `detail`).
local function edit_of(x)
  local d = type(x.detail) == "table" and x.detail or {}
  local function str(v) return type(v) == "string" and v or nil end
  local hs = field_of(d, { "headers", "config.headers" })
  local headers = {}
  if type(hs) == "table" then
    if #hs > 0 then
      for _, h in ipairs(hs) do if type(h) == "string" and trim(h) ~= "" then headers[#headers + 1] = h end end
    else
      local names = {}
      for k, v in pairs(hs) do if type(k) == "string" and type(v) == "string" then names[#names + 1] = k end end
      table.sort(names)
      for _, k in ipairs(names) do headers[#headers + 1] = k .. ": " .. hs[k] end
    end
  elseif type(hs) == "string" then
    for h in string.gmatch(hs, "[^;\n]+") do if trim(h) ~= "" then headers[#headers + 1] = h end end
  end
  local args = field_of(d, { "args", "config.args" })
  if type(args) == "table" then
    local words = {}
    for _, w in ipairs(args) do if type(w) == "string" then words[#words + 1] = w end end
    args = table.concat(words, " ")
  end
  return { name = x.name, stdio = x.stdio, url = str(field_of(d, { "url", "endpoint", "remote.url", "config.url" })),
    headers = headers, command = str(field_of(d, { "command", "config.command" })), args = str(args),
    dir = str(field_of(d, { "dir", "cwd", "config.dir" })) }
end

local function form_of(state, a)
  if (a and a.reset) or not state.mcp then state.mcp = {} end
  return state.mcp
end

local function header_row(f, id)
  for i, r in ipairs(f.rows or {}) do if r.id == id then return r, i end end
  return nil
end

return {
  init = function() return { mcp = nil, more = 0 } end,

  render = function(state, data)
    local errors = (data.app and data.app.errors) or {}
    local m = data.mcp or {}
    local err = errors.mcp or failed(data.mcp)
    local servers = {}
    for _, x in ipairs(m.servers or {}) do servers[#servers + 1] = { name = x.name, type = x.type, stdio = is_stdio(x), detail = x.detail } end

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
    local headers, header_bad = header_lines(f.rows)
    local keys = placeholders(headers)
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
      form[#form + 1] = cell("URL", input{ id = "mcp-url", value = f.url or "", on = "field", arg = { k = "url", reset = done }, placeholder = "https://mcp.example.com/mcp" })
      -- The header rows: name (picked, or typed for Custom), value, remove;
      -- they wrap when narrow. No row is the default (OAuth servers need none).
      local hrows = { grow = 1, min_w = 0, gap = 8, key = "mcp-headers" }
      local seen = {}
      for _, r in ipairs(f.rows or {}) do
        local id = tostring(r.id)
        local n = row_name(r)
        local why = row_problem(r, seen)
        if n ~= "" and trim(r.value) ~= "" then seen[string.lower(n)] = true end
        hrows[#hrows + 1] = column{ w = "full", gap = 4, key = "mcp-h-" .. id,
          row{ w = "full", gap = 8, wrap = true,
            select{ id = "mcp-h-name-" .. id, value = r.pick or HEADERS[1], items = HEADER_ITEMS, on = "header_pick", arg = { id = r.id, reset = done }, w = 160 },
            r.pick == CUSTOM and input{ id = "mcp-h-custom-" .. id, value = r.name or "", on = "header_field", arg = { id = r.id, k = "name", reset = done },
              placeholder = "Header name", w = 160 } or false,
            row{ gap = 8, grow = 1, min_w = 200,
              input{ id = "mcp-h-value-" .. id, value = r.value or "", on = "header_field", arg = { id = r.id, k = "value", reset = done },
                placeholder = value_hint(n), grow = 1, min_w = 0 },
              button{ icon = "close", variant = "ghost", on = "header_rm", arg = { id = r.id, reset = done }, key = "mcp-h-rm-" .. id, help = "Remove this header" } } },
          why and row{ w = "full", gap = 4, key = "mcp-h-why-" .. id,
            icon{ "warning", size = 12, color = "red" },
            text{ why, wrap = true, size = "sm", color = "red", grow = 1, min_w = 0 } } or false,
        }
      end
      if #(f.rows or {}) > 0 then
        hrows[#hrows + 1] = text{ "A secret goes in a value as ${NAME}: it is asked in the next dialog, never typed here.", wrap = true, size = "sm", color = "text-muted", key = "mcp-h-note" }
      end
      hrows[#hrows + 1] = row{ gap = 8, button{ "Add header", icon = "plus", variant = "ghost", on = "header_add", arg = { reset = done }, key = "mcp-h-add" } }
      -- The label stays beside the first row (top), not in the middle of many.
      form[#form + 1] = row{ w = "full", gap = 8, align = "start",
        row{ w = 76, h = 28, text{ "Headers", size = "sm", color = "text-muted" } }, column(hrows) }
    end
    if f.replace and f.unlisted then
      form[#form + 1] = note("sbx did not list this server's settings: fill them in again (Inspect shows its headers).", "amber")
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
      button{ f.replace and "Save…" or "Add…", key = "mcp-save", variant = "primary", on = "save", disabled = trim(f.name) == "" or (not stdio and header_bad ~= nil),
        help = (not stdio and header_bad) and "A header cannot be sent as typed: see the reason under it" or nil },
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
      -- Narrow, the actions take their own line and wrap inside it.
      local acts = row{ gap = 4, wrap = true, grow = (not wide) and 1 or nil, min_w = 0,
        button{ "Load", key = "mcp-load-" .. x.name, variant = "ghost", on = "load", arg = { server = x.name, sandbox = target }, disabled = not target,
          help = "Attach to " .. (target or "a running sandbox") .. ": its agent picks up the tools without a restart" },
        button{ "Unload", key = "mcp-unload-" .. x.name, variant = "ghost", on = "unload", arg = { server = x.name, sandbox = target }, disabled = not target },
        (not x.stdio) and button{ "Sign in", key = "mcp-auth-" .. x.name, variant = "ghost", on = "auth", arg = x.name,
          help = "sbx mcp auth: sign in from this machine's browser; the token serves every sandbox" } or false,
        button{ "Edit", key = "mcp-edit-" .. x.name, variant = "ghost", on = "edit", arg = edit_of(x),
          help = "Fill the form with this server's settings: saving removes it and adds it again (its OAuth token is dropped)" },
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
    header_add = function(state, a)
      local f = form_of(state, a)
      f.rows = f.rows or {}
      f.next = (f.next or 0) + 1
      f.rows[#f.rows + 1] = { id = f.next, pick = HEADERS[1], name = "", value = "" }
    end,
    header_rm = function(state, a)
      local f = form_of(state, a)
      local _, i = header_row(f, a.id)
      if i then table.remove(f.rows, i) end
    end,
    header_pick = function(state, a, v)
      local r = header_row(form_of(state, a), a.id)
      if r then r.pick = v end
    end,
    header_field = function(state, a, v)
      local r = header_row(form_of(state, a), a.id)
      if r then r[a.k] = v end
    end,
    kind = function(state, a, v)
      if a.reset or not state.mcp then state.mcp = {} end
      state.mcp.kind = v
    end,
    -- Edit: the form holds a listed server's settings and saves with
    -- `replace` (the core's dialog says it is removed and added again).
    edit = function(state, x)
      local f = { replace = x.name, name = x.name, kind = x.stdio and "stdio" or "remote",
        url = x.url or "", command = x.command or "", args = x.args or "", dir = x.dir or "", rows = {}, next = 0 }
      if not x.stdio then
        for _, h in ipairs(x.headers or {}) do
          f.next = f.next + 1
          f.rows[#f.rows + 1] = row_of(h, f.next)
        end
      end
      f.unlisted = (x.stdio and f.command == "") or (not x.stdio and f.url == "")
      state.mcp = f
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
        local headers, bad = header_lines(f.rows)
        -- Refused in the form (the reason is under the row): never sent otherwise than typed.
        if bad then return nil end
        if headers ~= "" then args.headers = headers end
      end
      if f.replace then args.replace = f.replace end
      f.sent = args.name
      state.mcp = f
      return intent("mcp_save", args)
    end,
  },
}
