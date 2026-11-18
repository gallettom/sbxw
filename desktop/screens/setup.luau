-- Setup (Settings in the rail): what sbxw holds for every sandbox, as tabs
-- of one dialog (assets/js/setup.js): Secrets, Hosts, Voice. MCP servers,
-- Kits and Skills have their own panels (mcp.luau, kits.luau, skills.luau)
-- and rail entries. The core reads the daemon and hands its answers; this
-- screen only shows them and asks for things with intents.
--
-- Secrets: this screen never sees a value. It shows names and whether a
-- value is set; "Set" asks the core, which draws its own dialog with a
-- secret field, sends the value once and forgets it. Everything that
-- changes what sandboxes get (a secret, a remote host, the voice models) is
-- confirmed in the core's dialog, not here.

local TABS = {
  { id = "secrets", label = "Secrets and sign-in" },
  { id = "hosts", label = "Remote hosts" },
  { id = "voice", label = "Voice" },
}

-- The daemon's answers (one key per route) as the tabs read them. Why a
-- key's last read failed is in `data.app.errors[key]` (the key keeps its
-- last good value); an answer `{ ok = false, error = ... }` says it too.
local function failed(v)
  if type(v) == "table" and v.ok == false then return v.error or "the daemon refused" end
  return nil
end

local function view(data)
  local d = { sandboxes = data.sandboxes or {}, errors = {} }
  local app_errors = (data.app and data.app.errors) or {}
  local function err(key) return app_errors[key] or failed(data[key]) end
  local sec = data.secrets
  local services, custom = {}, {}
  if type(sec) == "table" then
    for _, x in ipairs(sec.secrets or {}) do
      if x.kind == "service" then services[#services + 1] = x else custom[#custom + 1] = x end
    end
  end
  local gh = data.github_token
  d.secrets = { github = (type(gh) == "table" and gh.ok ~= false) and gh or nil, services = services, custom = custom }
  d.errors.secrets = err("secrets") or err("github_token")

  local rh = data.remote_hosts or {}
  d.hosts = { hosts = rh.hosts or {}, provision = data.provision }
  d.errors.hosts = err("remote_hosts")

  local v = data.voice
  if failed(v) or (v == nil and app_errors.voice) then
    d.voice = { unavailable = "This daemon has no voice: " .. (failed(v) or app_errors.voice) }
  elseif v == nil then
    d.voice = nil
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
-- data.app of the render under way (set by render, read by the tabs).
local data_app = nil

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
      button{ gh.set and "Replace token…" or "Set token…", key = "github-set", icon = "plus", variant = "primary", on = "github_set",
        help = "The core asks for the token in its own dialog; this window never holds it" },
      gh.gh and button{ "Use my gh login", key = "github-gh", on = "github_gh",
        help = "sbx runs " .. gh.gh .. " auth token when it needs the token, and refreshes it" } or false,
      spacer{},
      gh.set and button{ "Remove", key = "github-rm", icon = "trash", variant = "danger", on = "github_rm" } or false,
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
      button{ "Add…", key = "secret-add", icon = "plus", on = "secret_add" },
    },
  }
  -- Files dropped on a terminal: only the upload dialog's own box stops it
  -- asking; this screen can only turn the asking back on.
  local ask = not (data_app and data_app.ask_dropped_uploads == false)
  out[#out + 1] = card{ key = "setup-drops",
    title("Dropped files", pill(ask, "asked first", "not asked")),
    note(ask and "Files you drop on a terminal are uploaded into its sandbox after a dialog lists them."
      or "You chose not to be asked: files you drop on a terminal upload at once, and a notice says so. Files that look like secrets, come from ~/.ssh, ~/.aws or ~/.gnupg, or more than 10 files or 25 MB are asked about all the same."),
    not ask and row{ w = "full", spacer{},
      button{ "Ask before uploading dropped files", key = "setup-drops-ask", on = "drops_ask" } } or false,
  }
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
      row{ w = "full", spacer{}, button{ "Copy the command", icon = "copy", key = "provision-copy", on = "provision_copy", arg = hs.provision.command } },
    } or false,
  }

  local f = state.host or {}
  local done = false
  if f.sent and not f.editing then
    for _, h in ipairs(hs.hosts or {}) do if h.name == f.sent then done = true end end
  end
  if done then f = {} end
  local alias = f.mode ~= "direct"
  local form = {
    title(f.editing and ("Edit " .. f.editing) or "Add a host"),
    field("Name", input{ id = "host-name", value = f.name or "", on = "host_field", arg = { k = "name", reset = done }, placeholder = "build-box",
      disabled = f.editing ~= nil }),
    field("Connect by", select{ id = "host-mode", value = alias and "alias" or "direct", on = "host_mode", arg = { reset = done }, items = {
      { id = "alias", label = "An alias from ~/.ssh/config" },
      { id = "direct", label = "Address, user, port, key" } } }),
  }
  if alias then
    form[#form + 1] = field("SSH alias", input{ id = "host-ssh", value = f.ssh or "", on = "host_field", arg = { k = "ssh", reset = done }, placeholder = "build-box" })
  else
    form[#form + 1] = field("Address", input{ id = "host-host", value = f.host or "", on = "host_field", arg = { k = "host", reset = done }, placeholder = "10.0.0.12" })
    form[#form + 1] = field("User", input{ id = "host-user", value = f.user or "", on = "host_field", arg = { k = "user", reset = done }, placeholder = "optional" })
    form[#form + 1] = field("Port", input{ id = "host-port", value = f.port or "", on = "host_field", arg = { k = "port", reset = done }, placeholder = "22" })
    form[#form + 1] = field("Key file", input{ id = "host-key", value = f.key or "", on = "host_field", arg = { k = "key", reset = done }, placeholder = "~/.ssh/id_ed25519" })
  end
  form[#form + 1] = field("Agent port", input{ id = "host-agent-port", value = f.agent_port or "", on = "host_field", arg = { k = "agent_port", reset = done }, placeholder = "7682" })
  form[#form + 1] = field("sbx path", input{ id = "host-sbx-path", value = f.sbx_path or "", on = "host_field", arg = { k = "sbx_path", reset = done }, placeholder = "optional" })
  form[#form + 1] = checkbox{ "Trust this host with secrets", value = f.trust == true, on = "host_trust", arg = { reset = done } }
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
      badge{ m.role or "?" },
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
      button{ "Cancel", key = "voice-cancel", on = "voice_cancel" },
    }
  else
    if job and job.state == "failed" then children[#children + 1] = note("Last download failed: " .. (job.error or "unknown error"), "red") end
    if job and job.state == "cancelled" then children[#children + 1] = note("Download cancelled: installing resumes where it stopped.") end
    if not v.ready and v.compiled then
      children[#children + 1] = note("About " .. gb(v.approx_mb) .. " of models, into " .. (v.dir or "the models folder") .. ". From a terminal: " .. (v.hint or "sbxw voice install"))
    end
    children[#children + 1] = row{ w = "full", gap = 8,
      (not v.ready and v.compiled) and button{ "Install voice (~" .. gb(v.approx_mb) .. ")…", key = "voice-install", icon = "plus", variant = "primary", on = "voice_install" } or false,
      v.ready and button{ "Load the models now", on = "voice_warm", help = "Loads the recogniser (and the voice) before the first word" } or false,
      spacer{},
      v.has_files and button{ "Remove models…", key = "voice-rm", icon = "trash", variant = "danger", on = "voice_rm" } or false,
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

local BODY = { secrets = secrets_tab, hosts = hosts_tab, voice = voice_tab }

return {
  -- No tab yet: the one the core passes (the last one shown, kept across runs).
  init = function() return { tab = nil } end,

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
    tabs[#tabs + 1] = button{ icon = "reload", key = "setup-reload", variant = "ghost", on = "reload", arg = tab, help = "Read it again" }
    tabs[#tabs + 1] = button{ icon = "close", key = "setup-close", variant = "ghost", on = "close", help = "Close" }

    data_app = data.app
    local body = BODY[tab](state, d)
    body.key = "setup-body"
    local err = (d.errors or {})[tab]
    return column{ w = "full", h = "full", gap = 8,
      row(tabs),
      divider{},
      err and row{ w = "full", gap = 8, key = "setup-error",
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
    drops_ask = function() return intent("files_upload_ask_again") end,
    github_gh = function() return intent("github_token_gh") end,
    github_rm = function() return intent("github_token_rm") end,
    secret_hosts = function(state, _, v) state.secret_hosts = v end,
    secret_env = function(state, _, v) state.secret_env = v end,
    secret_add = function(state)
      return intent("secret_custom_set", { hosts = trim(state.secret_hosts), env = trim(state.secret_env) })
    end,
    secret_rm = function(_, name) return intent("secret_rm", { name = name }) end,

    -- Hosts
    host_field = function(state, a, v)
      if a.reset or not state.host then state.host = {} end
      state.host[a.k] = v
    end,
    host_mode = function(state, a, v)
      if a.reset or not state.host then state.host = {} end
      state.host.mode = v
    end,
    host_trust = function(state, a, on)
      if a.reset or not state.host then state.host = {} end
      state.host.trust = on
    end,
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
    provision_copy = function(_, cmd) return intent("copy_text", { text = cmd }) end,
    host_save = function(state)
      state.host = state.host or {}
      state.host.sent = trim(state.host.name)
      return intent("host_save", host_args(state.host))
    end,

    -- Voice
    voice_install = function() return intent("voice_install") end,
    voice_cancel = function() return intent("voice_cancel") end,
    voice_rm = function() return intent("voice_rm") end,
    voice_warm = function() return intent("voice_warm") end,
  },
}
