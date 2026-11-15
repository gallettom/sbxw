-- Setup: what every sandbox starts with and what sbxw holds for them, as tabs
-- of one dialog (assets/js/setup.js): Secrets, Kits, Skills, MCP, Hosts,
-- Voice. The core reads the daemon and hands `data.setup`; this screen only
-- shows it and asks for things with intents.
--
-- Secrets: this screen never sees a value. It shows names and whether a
-- value is set; "Set" asks the core, which draws its own dialog with a
-- secret field, sends the value once and forgets it. Everything that
-- changes what sandboxes get (a secret, a kits line, a kept skill, an MCP
-- server, a remote host, the voice models) is confirmed in the core's
-- dialog, not here.

local TABS = {
  { id = "secrets", label = "Secrets" },
  { id = "kits", label = "Kits" },
  { id = "skills", label = "Skills" },
  { id = "mcp", label = "MCP" },
  { id = "hosts", label = "Hosts" },
  { id = "voice", label = "Voice" },
}

-- The daemon's answers (one key per route) as the tabs read them. A key
-- whose read failed holds `{ ok = false, error = ... }`.
local function failed(v)
  if type(v) == "table" and v.ok == false then return v.error or "the daemon refused" end
  return nil
end

local function is_stdio(x)
  local t = string.lower(x.type or "")
  return string.find(t, "stdio") ~= nil or string.find(t, "local") ~= nil or string.find(t, "command") ~= nil
    or (type(x.detail) == "table" and x.detail.command ~= nil)
end

local function view(data)
  local d = { sandboxes = data.sandboxes or {}, errors = {} }
  local sec = data.secrets
  local services, custom = {}, {}
  if type(sec) == "table" then
    for _, x in ipairs(sec.secrets or {}) do
      if x.kind == "service" then services[#services + 1] = x else custom[#custom + 1] = x end
    end
  end
  local gh = data.github_token
  d.secrets = { github = (type(gh) == "table" and gh.ok ~= false) and gh or nil, services = services, custom = custom }
  d.errors.secrets = failed(sec) or failed(gh)

  d.kits = data.kits or {}
  d.errors.kits = failed(data.kits)

  local kept_names = {}
  local ms = data.my_skills or {}
  for _, x in ipairs(ms.skills or {}) do kept_names[x.name] = true end
  local look = nil
  if data.skills_for then
    local found = data.sandbox_skills
    look = { sandbox = data.skills_for, loading = found == nil }
    if type(found) == "table" then
      if found.ok == false then
        look.error = found.error
      else
        look.skills = {}
        for _, x in ipairs(found.skills or {}) do
          local c = {}
          for k, v in pairs(x) do c[k] = v end
          c.kept = kept_names[x.name] == true
          look.skills[#look.skills + 1] = c
        end
      end
    end
  end
  d.skills = { kept = ms.skills or {}, look = look }
  d.errors.skills = failed(data.my_skills) or (look and look.error)

  local servers = {}
  for _, x in ipairs((data.mcp or {}).servers or {}) do
    servers[#servers + 1] = { name = x.name, type = x.type, stdio = is_stdio(x) }
  end
  local inspect = nil
  if data.mcp_inspect_for then
    local t = data.mcp_inspect
    inspect = { server = data.mcp_inspect_for,
      text = type(t) == "table" and (t.text or t.error or "") or "Reading…" }
  end
  d.mcp = { servers = servers, inspect = inspect }
  d.errors.mcp = failed(data.mcp)

  local rh = data.remote_hosts or {}
  d.hosts = { hosts = rh.hosts or {}, provision = data.provision }
  d.errors.hosts = failed(data.remote_hosts)

  local v = data.voice
  if v == nil then
    d.voice = nil
  elseif failed(v) then
    d.voice = { unavailable = "This daemon has no voice: " .. failed(v) }
  else
    d.voice = v
    local files = false
    for _, m in ipairs(v.models or {}) do
      local st = (m.installed or {}).state
      if st == "ready" or st == "partial" then files = true end
    end
    d.voice.has_files = files
    d.voice.key = v.push_to_talk_key
  end
  return d
end

local function trim(v) return (string.gsub(v or "", "^%s*(.-)%s*$", "%1")) end

local function contains(list, v)
  for _, x in ipairs(list or {}) do if x == v then return true end end
  return false
end

local function note(t, color)
  return text{ t, wrap = true, size = "sm", color = color or "text-muted" }
end

local function title(t, extra)
  return row{ w = "full", gap = 8,
    text{ t, weight = "semibold", color = "text-bright" },
    extra or false,
  }
end

local function card(children)
  children.w = "full"
  children.gap = children.gap or 8
  children.pad = 12
  children.radius = "md"
  children.border = "border"
  children.fill = "surface"
  return column(children)
end

local function pill(on, yes, no)
  return badge{ on and yes or no, tone = on and "green" or "neutral" }
end

local function field(label, child)
  return row{ w = "full", gap = 8,
    text{ label, size = "sm", color = "text-muted", w = 96 },
    column{ grow = 1, child },
  }
end

local function mb(n) return tostring(math.floor((tonumber(n) or 0) / 1e6 + 0.5)) .. " MB" end
local function gb(m) return string.format("%.1f GB", (tonumber(m) or 0) / 1000) end
local function kb(n)
  n = tonumber(n) or 0
  if n < 1024 then return n .. " B" end
  return math.floor(n / 1024 + 0.5) .. " KB"
end

-- ── Secrets ─────────────────────────────────────────────────────────────
local function secrets_tab(state, d)
  local sec = d.secrets or {}
  local gh = sec.github
  local out = { gap = 12 }

  local services = {}
  for _, x in ipairs(sec.services or {}) do
    services[#services + 1] = row{ w = "full", gap = 8, key = "service-" .. x.name,
      icon{ "check", size = 12, color = "green" },
      text{ x.name, mono = true },
      spacer{},
      text{ x.scope == "global" and "every sandbox" or ("only " .. x.scope), size = "sm", color = "text-muted" },
    }
  end
  out[#out + 1] = card{
    title("Sign-in and services", pill(#services > 0, #services .. " stored", "none")),
    note("What sbx holds for the services it knows (Anthropic, GitHub, ...): the proxy presents them, no sandbox sees them. Names only; values are never read back."),
    #services > 0 and list(services) or note("No service credential stored. Claude sandboxes then ask you to /login in their session."),
  }

  local gh_children = {
    title("GitHub", gh and pill(gh.set, "token set", "no token") or badge{ "unknown", tone = "amber" }),
    note("The token sbx's proxy presents to github.com and api.github.com for every sandbox's git and gh. It never enters a sandbox."),
  }
  if gh then
    gh_children[#gh_children + 1] = row{ w = "full", gap = 8,
      button{ gh.set and "Replace token…" or "Set token…", icon = "plus", variant = "primary", on = "github_set",
        help = "The core asks for the token in its own dialog; this window never holds it" },
      gh.gh and button{ "Use my gh login", on = "github_gh",
        help = "sbx runs " .. gh.gh .. " auth token when it needs the token, and refreshes it" } or false,
      spacer{},
      gh.set and button{ "Remove", icon = "trash", variant = "danger", on = "github_rm" } or false,
    }
    gh_children[#gh_children + 1] = note(gh.gh and "sbx can ask gh for the token and keep it fresh."
      or "Install gh and run gh auth login to skip pasting.")
  end
  out[#out + 1] = card(gh_children)

  local custom = {}
  for _, x in ipairs(sec.custom or {}) do
    custom[#custom + 1] = row{ w = "full", gap = 8, key = "secret-" .. x.name,
      text{ x.name, mono = true, truncate = true, grow = 1 },
      text{ x.scope == "global" and "every sandbox" or ("only " .. x.scope), size = "sm", color = "text-muted" },
      x.scope == "global" and button{ "Remove", variant = "danger", on = "secret_rm", arg = x.name } or false,
    }
  end
  out[#out + 1] = card{
    title("Other services", pill(#custom > 0, #custom .. " stored", "none")),
    note("A key for a service sbx does not know: the sandbox sees a placeholder in the variable, the proxy swaps in the value on requests to these hosts only."),
    #custom > 0 and list(custom) or false,
    field("Hosts", input{ id = "secret-hosts", value = state.secret_hosts or "", on = "secret_hosts",
      placeholder = "api.example.com, *.example.io" }),
    field("Variable", input{ id = "secret-env", value = state.secret_env or "", on = "secret_env",
      placeholder = "EXAMPLE_API_KEY" }),
    row{ w = "full", gap = 8,
      note("The value is typed in the next dialog, drawn by sbxw."),
      spacer{},
      button{ "Add…", icon = "plus", on = "secret_add" },
    },
  }
  return out
end

-- ── Kits ────────────────────────────────────────────────────────────────
local function kits_file(d, state)
  local k = d.kits or {}
  for _, f in ipairs(k.files or {}) do
    if f.path == state.kits_path then return f end
  end
  for _, f in ipairs(k.files or {}) do
    if f.daemon then return f end
  end
  return (k.files or {})[1]
end

-- The line being edited: the person's edits, or the file's own line.
local function kits_list(d, state, f)
  if state.kits_edit and state.kits_edit.path == f.path then return state.kits_edit.list end
  if f.kits then return f.kits end
  return (d.kits or {}).defaults or {}
end

local function kit_label(ref)
  if string.find(ref, "/kits/jarvis%-") then return "Jarvis kit" end
  if string.find(ref, "^[%./~]") then return "local kit" end
  return false
end

local function kits_tab(state, d)
  local k = d.kits or {}
  local out = { gap = 12 }
  local waiting = {}
  for _, sb in ipairs(d.sandboxes or {}) do
    if (sb.kits_waiting or 0) > 0 then
      waiting[#waiting + 1] = row{ w = "full", gap = 8, key = "waiting-" .. sb.name,
        icon{ "warning", size = 12, color = "amber" },
        text{ sb.name, weight = "medium" },
        text{ sb.kits_waiting .. (sb.kits_waiting == 1 and " kit waits" or " kits wait") .. " for your answer", size = "sm", color = "text-muted" },
        spacer{},
        button{ "Answer in a pane", icon = "terminal", on = "kit_add", arg = sb.name,
          help = "Opens sbx kit add in a pane: answer sbx's prompt there, one kit per answer" },
      }
    end
  end
  if #waiting > 0 then
    out[#out + 1] = card{ title("Waiting for you"),
      note("A kit that adds network rules makes sbx ask you before it reaches a running sandbox."),
      list(waiting) }
  end

  local files = k.files or {}
  if #files == 0 then
    out[#out + 1] = note("The daemon runs without an sbxw.toml and no sandbox records one: start it from a project with sbxw web, or create a sandbox from one.")
    return out
  end
  local f = kits_file(d, state)
  local items = {}
  for _, x in ipairs(files) do items[#items + 1] = { id = x.path, label = x.path } end
  local cur = kits_list(d, state, f)
  local broken = f.error ~= nil
  local dirty = state.kits_edit ~= nil and state.kits_edit.path == f.path

  local users = {}
  if f.daemon then users[#users + 1] = "the daemon (new sandboxes from this app)" end
  for _, n in ipairs(f.sandboxes or {}) do users[#users + 1] = n end
  local lines = {}
  if #users > 0 then lines[#lines + 1] = "Used by " .. table.concat(users, ", ") .. "." end
  if not f.exists then lines[#lines + 1] = "The file does not exist yet: saving creates it." end
  if not f.kits then lines[#lines + 1] = "No kits line: the default kits apply. Saving writes them out." end

  local bundled = {}
  for _, b in ipairs(k.bundled or {}) do
    bundled[#bundled + 1] = row{ w = "full", gap = 8,
      checkbox{ b.ref, value = contains(cur, b.ref), on = "kit_toggle", arg = { ref = b.ref, path = f.path, list = cur }, disabled = broken, key = "kit-" .. b.name },
      b.default and badge{ "default", help = "What a file without a kits line gets" } or false,
      text{ b.about or "", size = "sm", color = "text-muted", truncate = true, grow = 1 },
    }
  end
  local others = {}
  for _, ref in ipairs(cur) do
    if string.sub(ref, 1, 5) ~= "sbxw:" then
      local lab = kit_label(ref)
      others[#others + 1] = row{ w = "full", gap = 8,
        text{ ref, mono = true, truncate = true, grow = 1, help = ref },
        lab and badge{ lab, tone = lab == "Jarvis kit" and "purple" or "neutral" } or false,
        button{ "Remove", variant = "ghost", on = "kit_remove", arg = { ref = ref, path = f.path, list = cur }, disabled = broken, key = "kit-rm-" .. ref },
      }
    end
  end

  out[#out + 1] = card{
    title("Kits line", dirty and badge{ "unsaved", tone = "amber" } or false),
    note("The kits line of each sbxw.toml sbxw uses. Only that line is rewritten, comments kept. A change reaches a sandbox at its next up."),
    select{ id = "kits-file", value = f.path, items = items, on = "kits_file" },
    #lines > 0 and note(table.concat(lines, " ")) or false,
    broken and note("Cannot be edited: " .. f.error, "red") or false,
    text{ "Bundled", size = "sm", weight = "semibold", color = "text-muted" },
    list(bundled),
    text{ "Other kits", size = "sm", weight = "semibold", color = "text-muted" },
    #others > 0 and list(others) or note("None: only bundled kits."),
    row{ w = "full", gap = 8,
      column{ grow = 1, input{ id = "kits-add", value = state.kit_add or "", on = "kit_typed", on_submit = "kit_push", arg = { path = f.path, list = cur },
        placeholder = "A path or a registry reference", disabled = broken } },
      button{ "Add", key = "kit-push", on = "kit_push", arg = { path = f.path, list = cur }, disabled = broken },
    },
    row{ w = "full", gap = 8,
      button{ "Use defaults", key = "kits-defaults", on = "kits_defaults", arg = f.path, disabled = broken or not f.kits,
        help = "Remove the kits line: its sandboxes then get the default kits" },
      spacer{},
      dirty and button{ "Discard", variant = "ghost", on = "kits_discard" } or false,
      button{ "Save…", key = "kits-save", variant = "primary", on = "kits_save", disabled = broken or not dirty },
    },
  }
  return out
end

-- ── Skills ──────────────────────────────────────────────────────────────
local function skills_tab(state, d)
  local sk = d.skills or {}
  local out = { gap = 12 }
  local kept = {}
  for _, x in ipairs(sk.kept or {}) do
    kept[#kept + 1] = row{ w = "full", gap = 8, align = "start", key = "kept-" .. x.name,
      column{ grow = 1, gap = 2,
        text{ "/" .. x.name, mono = true, color = "text-bright" },
        x.description and text{ x.description, size = "sm", wrap = true, color = "text-soft" } or false,
        text{ "from " .. (x.sandbox or "?"), size = "xs", color = "text-dim", help = x.path },
      },
      button{ "Remove", variant = "ghost", on = "skill_forget", arg = x.name, help = "New sandboxes stop getting it" },
    }
  end
  out[#out + 1] = card{
    title("Your skills", pill(#kept > 0, #kept .. " kept", "none")),
    note("Skills kept from a sandbox: every sandbox created from now on gets them. A sandbox can write a skill, never make it reach the others; keeping one is your act, here."),
    #kept > 0 and list(kept) or note("None kept yet."),
  }

  local names = {}
  for _, sb in ipairs(d.sandboxes or {}) do names[#names + 1] = { id = sb.name, label = sb.name } end
  local pick = state.skills_sandbox
  if not pick and #names > 0 then pick = names[1].id end
  local look = sk.look
  local found = false
  if look and look.sandbox == pick then
    if look.loading then
      found = note("Reading the sandbox's skills… (a stopped sandbox is started)")
    elseif look.skills then
      local rows = {}
      for _, x in ipairs(look.skills) do
        rows[#rows + 1] = row{ w = "full", gap = 8, align = "start", key = "found-" .. x.name,
          column{ grow = 1, gap = 2,
            text{ "/" .. x.name, mono = true, color = "text-bright" },
            x.description and text{ x.description, size = "sm", wrap = true, color = "text-soft" } or false,
            text{ x.path .. " · " .. (x.files or 0) .. " files, " .. kb(x.bytes), size = "xs", color = "text-dim" },
          },
          x.keepable and button{ x.kept and "Replace…" or "Keep…", key = "keep-" .. x.name, on = "skill_keep", arg = { sandbox = look.sandbox, name = x.name },
            help = "Every sandbox created from now on gets it" }
            or text{ "name not kept", size = "sm", color = "text-dim", help = "Lowercase letters, digits, - and _ only" },
        }
      end
      found = #rows > 0 and list(rows)
        or note("No skill in " .. look.sandbox .. ": none under the project's .claude/skills/ or the sandbox's own ~/.claude/skills/.")
    end
  end
  out[#out + 1] = card{
    title("Keep a skill from a sandbox"),
    row{ w = "full", gap = 8,
      column{ grow = 1, select{ id = "skills-sandbox", value = pick or "", items = names, on = "skills_sandbox",
        placeholder = "No sandbox", disabled = #names == 0 } },
      button{ "Look", icon = "search", on = "skills_look", arg = pick, disabled = not pick },
    },
    found,
  }
  return out
end

-- ── MCP ─────────────────────────────────────────────────────────────────
local function placeholders(headers)
  local keys = {}
  for k in string.gmatch(headers or "", "%${([%w_%-]+)}") do
    if not contains(keys, k) then keys[#keys + 1] = k end
  end
  return keys
end

local function mcp_tab(state, d)
  local m = d.mcp or {}
  local out = { gap = 12 }
  local running = {}
  for _, sb in ipairs(d.sandboxes or {}) do
    if sb.status == "running" then running[#running + 1] = { id = sb.name, label = sb.name } end
  end
  local target = state.mcp_sandbox
  if not contains((function() local l = {} for _, r in ipairs(running) do l[#l + 1] = r.id end return l end)(), target) then
    target = running[1] and running[1].id or nil
  end

  local rows = {}
  for _, x in ipairs(m.servers or {}) do
    rows[#rows + 1] = row{ w = "full", gap = 6, key = "mcp-" .. x.name,
      text{ x.name, mono = true, truncate = true, grow = 1, help = x.name },
      text{ x.type or "", size = "sm", color = "text-muted" },
      button{ "Load", key = "mcp-load-" .. x.name, variant = "ghost", on = "mcp_load", arg = { server = x.name, sandbox = target }, disabled = not target,
        help = "Attach to " .. (target or "a running sandbox") .. ": its agent picks up the tools without a restart" },
      button{ "Unload", key = "mcp-unload-" .. x.name, variant = "ghost", on = "mcp_unload", arg = { server = x.name, sandbox = target }, disabled = not target },
      (not x.stdio) and button{ "Sign in", variant = "ghost", on = "mcp_auth", arg = x.name,
        help = "sbx mcp auth: sign in from this machine's browser; the token serves every sandbox" } or false,
      button{ "Inspect", variant = "ghost", on = "mcp_inspect", arg = x.name },
      button{ icon = "trash", variant = "ghost", color = "red", on = "mcp_rm", arg = x.name, help = "Unregister from this machine" },
    }
  end
  out[#out + 1] = card{
    title("Servers on this machine", badge{ tostring(#(m.servers or {})) }),
    note("Registered on this machine (sbx mcp); a sandbox gets a server once it is loaded there, while it runs."),
    row{ w = "full", gap = 8,
      text{ "Sandbox", size = "sm", color = "text-muted" },
      column{ grow = 1, select{ id = "mcp-sandbox", value = target or "", items = running, on = "mcp_sandbox",
        placeholder = "no running sandbox", disabled = #running == 0 } },
    },
    #rows > 0 and list(rows) or note("No servers registered yet. Add the first one below."),
    m.inspect and column{ w = "full", gap = 4, pad = 8, radius = "sm", fill = "bg",
      row{ w = "full", text{ "sbx mcp inspect " .. m.inspect.server, size = "sm", color = "text-muted" }, spacer{},
        button{ icon = "close", variant = "ghost", on = "mcp_inspect_close", help = "Hide" } },
      text{ m.inspect.text ~= "" and m.inspect.text or "(no output)", mono = true, wrap = true, size = "sm" },
    } or false,
  }

  local f = state.mcp or {}
  local stdio = f.kind == "stdio"
  local keys = placeholders(f.headers)
  local form = {
    title(f.replace and ("Edit " .. f.replace) or "New server"),
    field("Name", input{ id = "mcp-name", value = f.name or "", on = "mcp_field", arg = "name", placeholder = "github" }),
    field("Kind", select{ id = "mcp-kind", value = stdio and "stdio" or "remote", on = "mcp_kind", items = {
      { id = "remote", label = "Remote (an HTTP endpoint)" },
      { id = "stdio", label = "Local command (runs on this machine)" } } }),
  }
  if stdio then
    form[#form + 1] = field("Command", input{ id = "mcp-command", value = f.command or "", on = "mcp_field", arg = "command", placeholder = "npx" })
    form[#form + 1] = field("Arguments", input{ id = "mcp-args", value = f.args or "", on = "mcp_field", arg = "args", placeholder = "-y @modelcontextprotocol/server-filesystem /srv" })
    form[#form + 1] = field("Directory", input{ id = "mcp-dir", value = f.dir or "", on = "mcp_field", arg = "dir", placeholder = "optional" })
    form[#form + 1] = note("A local command runs on this machine, outside any sandbox: the next dialog shows the exact line and asks you to confirm you read it.", "amber")
  else
    form[#form + 1] = field("URL", input{ id = "mcp-url", value = f.url or "", on = "mcp_field", arg = "url", placeholder = "https://mcp.example.com/mcp" })
    form[#form + 1] = field("Headers", input{ id = "mcp-headers", value = f.headers or "", on = "mcp_field", arg = "headers",
      placeholder = "Authorization: Bearer ${TOKEN}; one per ;" })
    if #keys > 0 then
      form[#form + 1] = note("Secrets asked in the next dialog: " .. table.concat(keys, ", ") .. (f.replace and " (left empty: unchanged)" or ""))
    end
    form[#form + 1] = note("OAuth servers are added unauthorized: use Sign in once added.")
  end
  form[#form + 1] = row{ w = "full", gap = 8,
    spacer{},
    (f.replace or trim(f.name) ~= "") and button{ "Clear", variant = "ghost", on = "mcp_clear" } or false,
    button{ f.replace and "Save…" or "Add…", variant = "primary", on = "mcp_save", disabled = trim(f.name) == "" },
  }
  out[#out + 1] = card(form)
  return out
end

-- ── Hosts ───────────────────────────────────────────────────────────────
local function link_label(h)
  if h.link == "up" then return "green", h.latency_ms and (h.latency_ms .. " ms") or "up" end
  if h.link == "down" then return "red", "down" end
  return "neutral", "never"
end

local function hosts_tab(state, d)
  local hs = d.hosts or {}
  local out = { gap = 12 }
  local rows = {}
  for _, h in ipairs(hs.hosts or {}) do
    local tone, word = link_label(h)
    local target = h.ssh and ("ssh " .. h.ssh) or ((h.user and (h.user .. "@") or "") .. (h.host or "") .. (h.port and (":" .. h.port) or ""))
    rows[#rows + 1] = row{ w = "full", gap = 8, key = "host-" .. h.name,
      dot{ tone = tone },
      text{ h.name, weight = "medium" },
      h.trust_secrets and badge{ "trusted with secrets", tone = "amber",
        help = "Sent the Anthropic token, the OAuth kit and your skills" } or false,
      text{ target .. " · " .. word .. " · " .. (h.sandboxes or 0) .. ((h.sandboxes == 1) and " sandbox" or " sandboxes"),
        size = "sm", color = "text-muted", truncate = true, grow = 1 },
      button{ "Test", key = "host-test-" .. h.name, variant = "ghost", on = "host_test", arg = h.name, help = "Run sbx version over the link, time a round trip" },
      button{ "Provision", key = "host-provision-" .. h.name, variant = "ghost", on = "host_provision", arg = h.name, help = "Install Docker's sbx on the host" },
      button{ "Edit", key = "host-edit-" .. h.name, variant = "ghost", on = "host_edit", arg = h },
      button{ icon = "trash", variant = "ghost", color = "red", on = "host_rm", arg = h.name, help = "Unregister this host" },
    }
  end
  out[#out + 1] = card{
    title("Remote hosts", badge{ "experimental", tone = "amber" }),
    note("Other machines sbxw runs sandboxes on, over SSH. Saved in ~/.sbxw/hosts.toml, the file sbxw host edits too."),
    #rows > 0 and list(rows) or note("No remote host yet. Add one below."),
    row{ w = "full", spacer{}, button{ "Read hosts.toml again", icon = "reload", variant = "ghost", on = "hosts_reload" } },
    hs.provision and column{ w = "full", gap = 4, pad = 8, radius = "sm", fill = "bg", key = "provision",
      row{ w = "full", note("To install sbx on " .. hs.provision.host .. ", run this in a terminal on this machine (sudo is asked there, never here):"),
        spacer{}, button{ icon = "close", variant = "ghost", on = "provision_close", help = "Hide" } },
      text{ hs.provision.command, mono = true, wrap = true, size = "sm" },
    } or false,
  }

  local f = state.host or {}
  local alias = f.mode ~= "direct"
  local form = {
    title(f.editing and ("Edit " .. f.editing) or "Add a host"),
    field("Name", input{ id = "host-name", value = f.name or "", on = "host_field", arg = "name", placeholder = "build-box",
      disabled = f.editing ~= nil }),
    field("Connect by", select{ id = "host-mode", value = alias and "alias" or "direct", on = "host_mode", items = {
      { id = "alias", label = "An alias from ~/.ssh/config" },
      { id = "direct", label = "Address, user, port, key" } } }),
  }
  if alias then
    form[#form + 1] = field("SSH alias", input{ id = "host-ssh", value = f.ssh or "", on = "host_field", arg = "ssh", placeholder = "build-box" })
  else
    form[#form + 1] = field("Address", input{ id = "host-host", value = f.host or "", on = "host_field", arg = "host", placeholder = "10.0.0.12" })
    form[#form + 1] = field("User", input{ id = "host-user", value = f.user or "", on = "host_field", arg = "user", placeholder = "optional" })
    form[#form + 1] = field("Port", input{ id = "host-port", value = f.port or "", on = "host_field", arg = "port", placeholder = "22" })
    form[#form + 1] = field("Key file", input{ id = "host-key", value = f.key or "", on = "host_field", arg = "key", placeholder = "~/.ssh/id_ed25519" })
  end
  form[#form + 1] = field("Agent port", input{ id = "host-agent-port", value = f.agent_port or "", on = "host_field", arg = "agent_port", placeholder = "7682" })
  form[#form + 1] = field("sbx path", input{ id = "host-sbx-path", value = f.sbx_path or "", on = "host_field", arg = "sbx_path", placeholder = "optional" })
  form[#form + 1] = checkbox{ "Trust this host with secrets", value = f.trust == true, on = "host_trust" }
  if f.trust then
    form[#form + 1] = note("sbxw then sends it your Anthropic token (CLAUDE_CODE_OAUTH_TOKEN or API key), the OAuth kit (your Claude login) and your skills. Anyone with root on the host can read them.", "amber")
  end
  form[#form + 1] = note("For a new address, sbxw first reads the host's SSH key; the next dialog shows its fingerprints. Trust them only if they are the host's.")
  local ready = trim(f.name) ~= "" and trim(f.mode == "direct" and f.host or f.ssh) ~= ""
  form[#form + 1] = row{ w = "full", gap = 8, spacer{},
    (f.editing or trim(f.name) ~= "") and button{ "Clear", variant = "ghost", on = "host_clear" } or false,
    button{ f.editing and "Save…" or "Add host…", key = "host-save", variant = "primary", on = "host_save", disabled = not ready },
  }
  out[#out + 1] = card(form)
  return out
end

-- ── Voice ───────────────────────────────────────────────────────────────
local function voice_tab(state, d)
  local v = d.voice
  local out = { gap = 12 }
  if not v then
    out[#out + 1] = note("Asking the daemon about the voice…")
    return out
  end
  if v.unavailable then
    out[#out + 1] = card{ title("Voice", badge{ "not available", tone = "amber" }), note(v.unavailable) }
    return out
  end
  local children = {
    title("Voice models", v.ready and badge{ "ready", tone = "green" } or badge{ "not installed", tone = "neutral" }),
    note("Speech becomes text on this machine (and answers are read aloud on a Mac with Apple Silicon). The audio never leaves it and is never kept."),
  }
  if not v.compiled then
    children[#children + 1] = note("This sbxw was built without voice. Rebuild it with: cargo build --release --features voice", "amber")
  end
  local models = {}
  for _, m in ipairs(v.models or {}) do
    local st = m.installed or {}
    local word, tone
    if st.state == "ready" then word, tone = "installed", "green"
    elseif st.state == "partial" then word, tone = "partial, " .. mb(st.have), "amber"
    else word, tone = "~" .. (m.approx_mb or 0) .. " MB", "neutral" end
    models[#models + 1] = row{ w = "full", gap = 8, key = "model-" .. (m.id or m.label),
      badge{ string.upper(m.role or "?") },
      text{ m.label or m.id, truncate = true, grow = 1 },
      text{ m.license or "", size = "sm", color = "text-dim" },
      badge{ word, tone = tone },
    }
  end
  if v.speech and v.speech.supported then
    models[#models + 1] = row{ w = "full", gap = 8, key = "model-runtime",
      badge{ "RUNTIME" },
      text{ (v.speech.runtime or "") .. ", fetched by uvx", truncate = true, grow = 1 },
      badge{ v.speech.runtime_ready and "ready" or "needs uv", tone = v.speech.runtime_ready and "green" or "amber" },
    }
  end
  if #models > 0 then children[#children + 1] = list(models) end

  local job = v.job
  local busy = job and (job.state == "running" or job.state == "verifying")
  if busy then
    local pct = (job.total and job.total > 0) and (job.done / job.total) or 0
    children[#children + 1] = progress{ value = pct, tone = "blue", key = "voice-progress" }
    children[#children + 1] = row{ w = "full", gap = 8,
      note((job.state == "verifying" and "Checking " or "Downloading ") .. (job.model or "") .. " · " .. (job.file or "")
        .. ": " .. math.floor(pct * 100) .. "% (" .. mb(job.done) .. " / " .. mb(job.total) .. ")"),
      spacer{},
      button{ "Cancel", on = "voice_cancel" },
    }
  else
    if job and job.state == "failed" then children[#children + 1] = note("Last download failed: " .. (job.error or "unknown error"), "red") end
    if job and job.state == "cancelled" then children[#children + 1] = note("Download cancelled: installing resumes where it stopped.") end
    if not v.ready and v.compiled then
      children[#children + 1] = note("About " .. gb(v.approx_mb) .. " of models, into " .. (v.dir or "the models folder") .. ". From a terminal: " .. (v.hint or "sbxw voice install"))
    end
    children[#children + 1] = row{ w = "full", gap = 8,
      (not v.ready and v.compiled) and button{ "Install voice (~" .. gb(v.approx_mb) .. ")…", icon = "plus", variant = "primary", on = "voice_install" } or false,
      v.ready and button{ "Load the models now", on = "voice_warm", help = "Loads the recogniser (and the voice) before the first word" } or false,
      spacer{},
      v.has_files and button{ "Remove models…", icon = "trash", variant = "danger", on = "voice_rm" } or false,
    }
  end
  if v.key then
    children[#children + 1] = note("Push to talk: " .. v.key .. ". Set in sbxw.toml, [voice] push_to_talk_key.")
  end
  out[#out + 1] = card(children)
  return out
end

-- The form as intent arguments; nil fields are left out.
local function host_args(f)
  local args = { name = trim(f.name) }
  local function put(k, v) v = trim(v); if v ~= "" then args[k] = v end end
  local function num(k, v) local n = tonumber(trim(v)); if n then args[k] = math.floor(n) end end
  if f.mode == "direct" then put("host", f.host); put("user", f.user); put("key", f.key); num("port", f.port)
  else put("ssh", f.ssh) end
  num("agent_port", f.agent_port)
  put("sbx_path", f.sbx_path)
  args.trust_secrets = f.trust == true
  if f.editing then args.editing = f.editing end
  return args
end

local BODY = { secrets = secrets_tab, kits = kits_tab, skills = skills_tab, mcp = mcp_tab, hosts = hosts_tab, voice = voice_tab }

return {
  init = function() return { tab = "secrets" } end,

  render = function(state, data)
    local d = view(data)
    local tab = state.tab or (data.app and data.app.modal and data.app.modal.arg) or "secrets"
    if not BODY[tab] then tab = "secrets" end
    local tabs = { w = "full", gap = 4 }
    for _, t in ipairs(TABS) do
      tabs[#tabs + 1] = button{ t.label, key = "tab-" .. t.id, on = "tab", arg = t.id,
        variant = t.id == tab and "neutral" or "ghost",
        color = t.id == tab and "text-bright" or "text-muted" }
    end
    tabs[#tabs + 1] = spacer{}
    tabs[#tabs + 1] = button{ icon = "reload", variant = "ghost", on = "reload", arg = tab, help = "Read it again" }
    tabs[#tabs + 1] = button{ icon = "close", variant = "ghost", on = "close", help = "Close" }

    local body = BODY[tab](state, d)
    body.key = "setup-body"
    local err = (d.errors or {})[tab]
    return column{ w = "full", h = "full", gap = 8,
      row(tabs),
      divider{},
      err and row{ w = "full", gap = 6, key = "setup-error",
        icon{ "warning", size = 12, color = "red" },
        column{ grow = 1, text{ err, wrap = true, size = "sm", color = "red" } },
      } or false,
      scroll(body),
    }
  end,

  actions = {
    tab = function(state, t) state.tab = t; return intent("setup_load", { section = t }) end,
    reload = function(_, t) return intent("setup_load", { section = t }) end,
    close = function() return intent("close_modal") end,

    -- Secrets
    github_set = function() return intent("github_token_set") end,
    github_gh = function() return intent("github_token_gh") end,
    github_rm = function() return intent("github_token_rm") end,
    secret_hosts = function(state, _, v) state.secret_hosts = v end,
    secret_env = function(state, _, v) state.secret_env = v end,
    secret_add = function(state)
      return intent("secret_custom_set", { hosts = trim(state.secret_hosts), env = trim(state.secret_env) })
    end,
    secret_rm = function(_, name) return intent("secret_rm", { name = name }) end,

    -- Kits: the line is edited here, saved through the core's dialog.
    kits_file = function(state, _, path) state.kits_path = path; state.kits_edit = nil end,
    kit_toggle = function(state, a, on)
      local list = {}
      for _, k in ipairs(a.list or {}) do
        if k ~= a.ref then list[#list + 1] = k end
      end
      if on then list[#list + 1] = a.ref end
      state.kits_path = a.path
      state.kits_edit = { path = a.path, list = list }
    end,
    kit_remove = function(state, a)
      local list = {}
      for _, k in ipairs(a.list or {}) do
        if k ~= a.ref then list[#list + 1] = k end
      end
      state.kits_path = a.path
      state.kits_edit = { path = a.path, list = list }
    end,
    kit_typed = function(state, _, v) state.kit_add = v end,
    kit_push = function(state, a)
      local ref = trim(state.kit_add)
      if ref == "" then return end
      local list = {}
      for _, k in ipairs(a.list or {}) do list[#list + 1] = k end
      if not contains(list, ref) then list[#list + 1] = ref end
      state.kits_path = a.path
      state.kits_edit = { path = a.path, list = list }
      state.kit_add = ""
    end,
    kits_discard = function(state) state.kits_edit = nil end,
    kits_save = function(state)
      if not state.kits_edit then return end
      local e = state.kits_edit
      return intent("kits_save", { path = e.path, kits = table.concat(e.list, "\n") })
    end,
    kits_defaults = function(state, path)
      state.kits_edit = nil
      return intent("kits_save", { path = path, defaults = true })
    end,
    kit_add = function(_, name)
      return intent("open_terminal", { sandbox = name, mode = "kit-add", pane = "spare" })
    end,

    -- Skills
    skills_sandbox = function(state, _, v) state.skills_sandbox = v end,
    skills_look = function(_, name) return intent("skills_look", { sandbox = name }) end,
    skill_keep = function(_, a) return intent("skill_keep", { sandbox = a.sandbox, name = a.name }) end,
    skill_forget = function(_, name) return intent("skill_forget", { name = name }) end,

    -- MCP
    mcp_sandbox = function(state, _, v) state.mcp_sandbox = v end,
    mcp_load = function(_, a) return intent("mcp_load", { server = a.server, sandbox = a.sandbox }) end,
    mcp_unload = function(_, a) return intent("mcp_unload", { server = a.server, sandbox = a.sandbox }) end,
    mcp_auth = function(_, server) return intent("mcp_auth", { server = server }) end,
    mcp_inspect = function(_, server) return intent("mcp_inspect", { server = server }) end,
    mcp_inspect_close = function() return intent("mcp_inspect_close") end,
    mcp_rm = function(_, server) return intent("mcp_rm", { server = server }) end,
    mcp_field = function(state, k, v) state.mcp = state.mcp or {}; state.mcp[k] = v end,
    mcp_kind = function(state, _, v) state.mcp = state.mcp or {}; state.mcp.kind = v end,
    mcp_clear = function(state) state.mcp = {} end,
    mcp_save = function(state)
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
      return intent("mcp_save", args)
    end,

    -- Hosts
    host_field = function(state, k, v) state.host = state.host or {}; state.host[k] = v end,
    host_mode = function(state, _, v) state.host = state.host or {}; state.host.mode = v end,
    host_trust = function(state, _, on) state.host = state.host or {}; state.host.trust = on end,
    host_clear = function(state) state.host = {} end,
    host_test = function(_, name) return intent("host_test", { name = name }) end,
    host_rm = function(_, name) return intent("host_rm", { name = name }) end,
    hosts_reload = function() return intent("hosts_reload") end,
    host_edit = function(state, h)
      state.host = { editing = h.name, name = h.name, mode = h.ssh and "alias" or "direct",
        ssh = h.ssh, host = h.host, user = h.user, port = h.port and tostring(h.port) or nil, key = h.key,
        agent_port = (h.agent_port and h.agent_port ~= 7682) and tostring(h.agent_port) or nil,
        sbx_path = h.sbx_path, trust = h.trust_secrets == true }
    end,
    host_provision = function(_, name) return intent("host_provision", { name = name }) end,
    provision_close = function() return intent("host_provision_close") end,
    host_save = function(state) return intent("host_save", host_args(state.host or {})) end,

    -- Voice
    voice_install = function() return intent("voice_install") end,
    voice_cancel = function() return intent("voice_cancel") end,
    voice_rm = function() return intent("voice_rm") end,
    voice_warm = function() return intent("voice_warm") end,
  },
}
