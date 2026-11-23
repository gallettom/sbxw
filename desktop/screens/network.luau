-- The Network panel (docked on the right): what sbx waits for you to
-- decide, then one sandbox's network policy (blocked hosts, rules, recent
-- egress) and its published ports. The web UI's approvals.js and ports.js.
--
-- Every answer and every change that widens access returns an intent the
-- core confirms in its own dialog, with the daemon's words; this screen only
-- asks.

local kinds = { network = "Network", port = "Host port" }

-- The sandbox shown, kept by render for the actions (they do not get `data`).
local current = nil

local function at(view, row, role)
  for i, r in ipairs(view.roles or {}) do
    if r == role then return row[i] or "" end
  end
  return ""
end

local function scope_label(v)
  v = v or ""
  if v == "" then return "?" end
  if string.lower(v) == "all" then return "all sandboxes" end
  if string.sub(string.lower(v), 1, 8) == "sandbox:" then return "this sandbox" end
  return v
end

local function governed(src)
  src = string.lower(src or "")
  return src == "org" or src == "corporate" or src == "system"
end

local function section(title, count)
  return row{ gap = 8, w = "full", pad = 2,
    text{ title, size = "caption", weight = "semibold", color = "text-muted" },
    count and badge{ tostring(count), tone = "neutral" } or false,
  }
end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function has_dismiss(r)
  for _, o in ipairs(r.options or {}) do
    if o.id == "dismiss" or string.lower(o.label or ""):sub(1, 7) == "dismiss" then return true end
  end
  return false
end

local function plural(n, w, ws) return n .. " " .. (n == 1 and w or (ws or (w .. "s"))) end

-- Cards drawn at a time: 113 requests of 33 hosts are 33 cards, and the
-- core builds the elements again each frame; past this, "Show more".
local PAGE = 25

-- The sandbox whose requests alone are listed (data.network_view.filter),
-- kept by render for the actions.
local filter_now = nil

local function requests(data, state)
  local a = data.approvals or {}
  local out = { gap = 8, w = "full" }
  local filter = data.network_view and data.network_view.filter or nil
  -- Only the requests of sandboxes in the list, as the web UI shows them:
  -- sbx lists the whole machine's (removed sandboxes, Jarvis's own).
  local listed = {}
  for _, sb in ipairs(data.sandboxes or {}) do listed[sb.name] = true end
  -- sbx lists a refused host again each time the sandbox retries it: one
  -- card per sandbox and host, the newest request's options, "×N". Under a
  -- filter, everything below counts only what is shown.
  local groups, order, hosts, sbs, nsb, nhosts, total, refused, all = {}, {}, {}, {}, 0, 0, 0, 0, {}
  for _, r in ipairs(a.approvals or {}) do
    local k = (r.sandbox or "") .. "\n" .. (r.kind or "") .. "\n" .. (r.title or "")
    if listed[r.sandbox] then all[k] = true end
    if listed[r.sandbox] and (not filter or r.sandbox == filter) then
      total = total + 1
      local g = groups[k]
      if not g then
        g = { first = r, n = 0, dismiss = false }
        groups[k] = g
        table.insert(order, g)
      end
      g.n = g.n + 1
      if (r.created_at or "") > (g.first.created_at or "") then g.first = r end
      if r.kind == "network" and has_dismiss(r) then g.dismiss = true; refused = refused + 1 end
      local hk = (r.kind or "") .. "\n" .. (r.title or "")
      if not hosts[hk] then hosts[hk] = true; nhosts = nhosts + 1 end
      if not sbs[r.sandbox] then sbs[r.sandbox] = true; nsb = nsb + 1 end
    end
  end
  local nall = 0
  for _ in pairs(all) do nall = nall + 1 end
  table.insert(out, row{ gap = 8, w = "full", pad = 2,
    text{ "Waiting for you", size = "caption", weight = "semibold", color = "text-muted" },
    badge{ tostring(#order), tone = "neutral" },
    filter and text{ "in", size = "caption", color = "text-muted" } or false,
    filter and button{ filter, icon = "close", dense = true, on = "filter_clear", key = "net-filter-chip",
      help = "Showing " .. filter .. "'s requests only. Remove the filter" } or false,
    spacer{},
    filter and button{ "Show all " .. nall, variant = "ghost", tone = "blue", dense = true, on = "filter_clear", key = "net-filter-all",
      help = "List the requests of every sandbox" } or false,
  })
  local err = (data.app.errors or {}).approvals
  if a.ok == false or err then
    table.insert(out, note(a.error or err or "Could not read what sbx is waiting for", "red"))
  end
  if #order == 0 and a.ok ~= false then
    if filter then
      table.insert(out, note("Nothing waits in " .. filter .. "." .. (nall > 0 and (" " .. plural(nall, "request") .. " of other sandboxes: Show all.") or "")))
    else
      table.insert(out, note("Nothing waiting: sbx has no pending request."))
    end
  end
  -- How many cards are drawn: PAGE, more on "Show more"; back to PAGE
  -- when the filter changes.
  if state.more_for ~= (filter or "") then state.more_for, state.more = filter or "", 0 end
  local shown = math.min(#order, PAGE * (1 + (state.more or 0)))
  if total > 1 then
    local where = filter and ("in " .. filter) or ("in " .. plural(nsb, "sandbox", "sandboxes"))
    table.insert(out, note(plural(total, "request") .. ": " .. plural(nhosts, "distinct host") .. " " .. where .. "."
      .. (shown < #order and (" Showing " .. shown .. " of " .. #order .. ".") or "")))
  end
  if refused > 1 then
    -- The bulk action takes exactly what its label says: the shown
    -- sandbox's refused hosts under a filter, the number checked again by
    -- the core (`expect`).
    -- "these" only when every card it takes is drawn; past the cap the
    -- label names the whole count (and the sandbox), as the note above.
    local label, what
    if filter then
      label = shown < #order and ("Dismiss all " .. refused .. " of " .. filter) or ("Dismiss these " .. refused)
    else
      label = "Dismiss all " .. refused
    end
    what = filter and (plural(refused, "refused host request") .. " of " .. filter) or ("every refused host listed here (" .. refused .. ")")
    table.insert(out, row{ gap = 8, w = "full",
      text{ plural(refused, "is a host", "are hosts") .. " sbx already refused: nothing waits on " .. (refused == 1 and "it" or "them") .. ".", size = "sm", color = "text-muted", wrap = true, grow = 1 },
      button{ label, icon = "close", on = "dismiss_all", arg = { sandbox = filter, expect = refused }, key = "approvals-dismiss-all",
        help = "Answer sbx's Dismiss on " .. what .. ", not a kit's port request. They stay refused; the sandbox's next try asks again" },
    })
  end
  for i, g in ipairs(order) do
    if i > shown then break end
    local r = g.first
    local buttons = { gap = 8, w = "full" }
    for _, o in ipairs(r.options or {}) do
      if g.dismiss and (o.id == "dismiss" or string.lower(o.label or ""):sub(1, 7) == "dismiss") then
        -- Dismissing is direct, and takes every copy of this host's request.
        table.insert(buttons, button{ (o.label ~= "" and o.label or o.id) .. (g.n > 1 and (" ×" .. g.n) or ""),
          on = "dismiss_group", arg = { sandbox = r.sandbox, title = r.title }, key = "answer-" .. r.id .. "-" .. o.id })
      else
        table.insert(buttons, button{ o.label ~= "" and o.label or o.id,
          -- Colour by meaning, not a filled button on every card: allow is
          -- green, refuse red; the one filled button is the dialog's.
          tone = (o.id == "allow") and "green" or ((o.id == "deny" or o.id == "refuse") and "red" or nil),
          on = "answer", arg = { id = r.id, option = o.id }, key = "answer-" .. r.id .. "-" .. o.id })
      end
    end
    table.insert(out, column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = r.kind == "network" and "elevated" or "amber", key = "req-" .. r.id,
      row{ gap = 8, w = "full",
        badge{ kinds[r.kind] or r.kind or "request", tone = r.kind == "network" and "neutral" or "amber" },
        text{ r.title or "", mono = true, color = "text-bright", truncate = true, grow = 1 },
        g.n > 1 and text{ "×" .. g.n, size = "sm", color = "text-muted", help = "sbx listed this " .. g.n .. " times (each retry)" } or false,
      },
      text{ (r.sandbox or "") .. ((r.detail and r.detail ~= "") and (" · " .. string.gsub(r.detail, "\n", " · ")) or ""), size = "sm", color = "text-muted", wrap = true },
      row(buttons),
    })
  end
  if shown < #order then
    table.insert(out, row{ gap = 8, w = "full", spacer{},
      button{ "Show " .. math.min(PAGE, #order - shown) .. " more", variant = "ghost", tone = "blue", on = "more", key = "net-req-more",
        help = (#order - shown) .. " more not drawn" } })
  end
  return column(out)
end

local function blocked_view(sb, policy)
  local hosts = policy.blocked or {}
  if #hosts == 0 then return false end
  local rows = {}
  for _, b in ipairs(hosts) do
    local action
    if b.governed then
      action = badge{ "org", tone = "neutral", help = b.reason ~= "" and b.reason or "Blocked by organisation policy" }
    elseif b.resource ~= "" then
      action = button{ "Allow", variant = "neutral", tone = "green", on = "allow_blocked", arg = b.host, help = "Allow " .. b.resource .. " for " .. sb, key = "allow-" .. b.host }
    else
      action = false
    end
    table.insert(rows, row{ gap = 8, w = "full",
      text{ b.host, mono = true, size = "sm", color = "text", truncate = true, grow = 1, help = b.reason },
      (b.count or 0) > 1 and text{ "×" .. b.count, size = "sm", color = "text-muted" } or false,
      action,
    })
  end
  return column{ gap = 4, w = "full", section("Blocked recently", #hosts), list(rows) }
end

local function rules_view(sb, policy, state)
  local v = policy.rules or {}
  local out = { gap = 4, w = "full" }
  local rows = v.rows or {}
  table.insert(out, section("Rules", #rows))
  if v.error then table.insert(out, note(v.error, "red")) end
  if #rows > 8 then
    table.insert(out, input{ id = "rule-filter", value = state.rule_filter, placeholder = "Filter rules: domain, decision, source", on = "rule_filter" })
  end
  local items = {}
  local q = string.lower(state.rule_filter or "")
  for _, r in ipairs(rows) do
    local line = table.concat(r, " ")
    if q == "" or string.find(string.lower(line), q, 1, true) then
      local action = string.lower(at(v, r, "action"))
      local id = at(v, r, "rule_id")
      local host = at(v, r, "host")
      local src = at(v, r, "source")
      local rm
      if id == "" or id == "-" then
        rm = false
      elseif governed(src) then
        rm = badge{ "org", tone = "neutral", help = "Set by organisation policy: sbx does not let sbxw remove it" }
      else
        rm = button{ icon = "close", variant = "ghost", tone = "red", on = action == "allow" and "rule_rm" or "unblock", arg = id,
          help = "Remove this rule", key = "rule-rm-" .. id }
      end
      table.insert(items, row{ gap = 8, w = "full",
        badge{ action ~= "" and action or "?", tone = action == "allow" and "green" or (action == "deny" and "red" or "neutral") },
        text{ host ~= "" and host or line, mono = true, size = "sm", color = "text", truncate = true, grow = 1 },
        text{ scope_label(at(v, r, "applies") ~= "" and at(v, r, "applies") or at(v, r, "sandbox")), size = "xs", color = "text-muted" },
        src ~= "" and text{ src, size = "xs", color = "text-muted" } or false,
        rm,
      })
    end
  end
  if #items > 0 then table.insert(out, list(items)) end
  if #rows == 0 and not v.error then table.insert(out, note("sbx reported no rules for this sandbox.")) end
  if (v.truncated or 0) > 0 then table.insert(out, note("+ " .. v.truncated .. " more not shown: run `sbx policy ls --wide`.")) end
  -- Add a rule.
  table.insert(out, column{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
    input{ id = "rule-res", value = state.res, placeholder = "example.com, *.acme.dev, host:443", on = "res", on_submit = "add_rule" },
    row{ gap = 8, w = "full",
      select{ id = "rule-decision", items = { { id = "allow", label = "allow" }, { id = "deny", label = "deny" } }, value = state.decision or "allow", on = "decision" },
      checkbox{ "all sandboxes", value = state.global or false, on = "global", key = "rule-global" },
      spacer{},
      button{ "Add rule", icon = "plus", tone = "green", on = "add_rule", disabled = (state.res or "") == "", key = "rule-add" },
    },
    checkbox{ "Only some HTTP requests", value = state.http or false, on = "http", key = "rule-http" },
    state.http and row{ gap = 8, w = "full",
      input{ id = "rule-methods", value = state.methods, placeholder = "GET, HEAD", on = "methods" },
      input{ id = "rule-path", value = state.path, placeholder = "/v1/**", on = "path" },
      input{ id = "rule-query", value = state.query, placeholder = "org=acme", on = "query" },
    } or false,
  })
  return column(out)
end

-- sbx's answer to "Why?" (data.network_why), under the host it was asked
-- for. Every word of it is sbx's, set as text. The remove button is the
-- rules list's own (the same intents), only for a rule id the panel lists.
local function why_view(w)
  local out = { gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = "elevated", key = "why-answer" }
  local head = row{ gap = 8, w = "full",
    text{ "Why " .. (w.target or "") .. "?", size = "sm", weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
    button{ icon = "close", variant = "ghost", dense = true, on = "why_close", key = "why-close", help = "Hide" },
  }
  table.insert(out, head)
  if w.busy then
    table.insert(out, note("Asking sbx…"))
  elseif w.error then
    table.insert(out, note(w.error, "red"))
  elseif w.supported == false then
    table.insert(out, note(w.note or "needs sbx 0.54", "text-muted"))
  else
    local verdict = w.allowed == true and badge{ "allowed", tone = "green" } or (w.allowed == false and badge{ "denied", tone = "red" } or badge{ "sbx does not say", tone = "neutral" })
    table.insert(out, row{ gap = 8, w = "full",
      verdict,
      text{ w.rule and ("Rule: " .. w.rule) or "No rule named", size = "sm", mono = w.rule ~= nil, color = "text", truncate = true, grow = 1, key = "why-rule" },
      (w.rule_id and w.removable) and button{ "Remove rule", icon = "close", variant = "ghost", tone = "red", on = w.removable == "rule_rm" and "rule_rm" or "unblock", arg = w.rule_id,
        key = "why-rm", help = "Remove rule " .. w.rule_id .. " (the rules list's remove)" } or false,
    })
    if (w.raw or "") ~= "" then
      table.insert(out, text{ w.raw, size = "xs", mono = true, color = "text-muted", wrap = true, key = "why-raw" })
    end
  end
  return column(out)
end

local function log_view(sb, policy, why)
  local v = policy.log or {}
  if v.error or not v.rows or #v.rows == 0 then return false end
  local items = {}
  for i, r in ipairs(v.rows) do
    if i > 12 then break end
    local action = string.lower(at(v, r, "action"))
    local host = at(v, r, "host")
    table.insert(items, row{ gap = 8, w = "full",
      text{ at(v, r, "time"), size = "xs", color = "text-muted" },
      badge{ action ~= "" and action or "?", tone = action == "allow" and "green" or (action == "deny" and "red" or "neutral") },
      text{ host, mono = true, size = "sm", color = "text", truncate = true, grow = 1 },
      (action == "allow" and host ~= "") and button{ "Why?", variant = "ghost", tone = "blue", dense = true, on = "why", arg = host, key = "why-" .. host,
        busy = (why and why.target == host and why.busy) and true or false,
        help = "Ask sbx which rule let " .. host .. " through (sbx policy check, sbx 0.54)" } or false,
    })
    if why and why.sandbox == sb and why.target == host then
      table.insert(items, why_view(why))
    end
  end
  return column{ gap = 4, w = "full", section("Recent egress", #v.rows), list(items) }
end

local function ports_view(sb, ports, hosts, state, pub)
  local list_ = (ports or {}).ports or {}
  local items = {}
  -- The last publish's answer for this sandbox (data.network_publish).
  if not (pub and pub.sandbox == sb) then pub = nil end
  for _, p in ipairs(list_) do
    local picked = pub and pub.assigned and pub.host_port == p.host_port and pub.sandbox_port == p.sandbox_port
    table.insert(items, row{ gap = 8, w = "full", key = "port-" .. p.spec,
      text{ tostring(p.sandbox_port), weight = "semibold", color = "text-bright" },
      text{ "→ " .. (p.host_ip or "") .. ":" .. tostring(p.host_port), mono = true, size = "sm", color = "text-muted", grow = 1 },
      picked and badge{ "sbx picked", tone = "blue", help = "sbx chose host port " .. tostring(p.host_port) .. " (no host port was given)", key = "port-picked-" .. p.spec } or false,
      badge{ p.proto or "tcp", tone = "neutral" },
      button{ "Open", icon = "link", variant = "ghost", tone = "blue", on = "port_open", arg = p.spec, key = "port-open-" .. p.spec, help = "Open in the browser" },
      button{ icon = "close", variant = "ghost", tone = "red", on = "port_rm", arg = p.spec, key = "port-rm-" .. p.spec, help = "Unpublish" },
    })
  end
  local out = { gap = 4, w = "full", section("Ports", #list_) }
  if #items > 0 then table.insert(out, list(items)) else table.insert(out, note("No ports published.")) end
  table.insert(out, column{ gap = 8, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
    row{ gap = 8, w = "full",
      input{ id = "port-sbx", value = state.sport, placeholder = "sandbox port", on = "sport" },
      input{ id = "port-host", value = state.hport, placeholder = "host port (sbx picks)", on = "hport" },
    },
    row{ gap = 8, w = "full",
      input{ id = "port-ip", value = state.hip, placeholder = "host address (127.0.0.1)", on = "hip" },
      input{ id = "port-alias", value = state.alias, placeholder = "alias (optional)", on = "alias" },
    },
    row{ gap = 8, w = "full", spacer{},
      button{ "Publish", icon = "plus", tone = "green", on = "publish", key = "port-publish",
        busy = (pub and pub.busy) and true or false, disabled = tonumber(state.sport or "") == nil and not (pub and pub.busy),
        help = "Leave the host port empty: sbx picks a free one (sbx 0.54)" },
    },
  })
  if pub and not pub.busy then
    if pub.error then
      table.insert(out, note("Not published: " .. pub.error, "red"))
    elseif pub.host_port then
      table.insert(out, text{ "Published " .. tostring(pub.sandbox_port) .. " on " .. (pub.host_ip or "") .. ":" .. tostring(pub.host_port)
        .. (pub.assigned and ", the port sbx picked." or ".") .. (pub.forwarded_port and (" Here: 127.0.0.1:" .. tostring(pub.forwarded_port) .. ".") or ""),
        size = "sm", color = "text-muted", wrap = true, key = "port-published" })
    end
    if pub.note then table.insert(out, note(pub.note, "amber")) end
  end
  local h = hosts or {}
  if #h > 0 then
    local hi = {}
    for _, e in ipairs(h) do
      table.insert(hi, row{ gap = 8, w = "full", text{ e.hostname or "", size = "sm", grow = 1 }, text{ e.ip or "", mono = true, size = "sm", color = "text-muted" } })
    end
    table.insert(out, text{ "/etc/hosts aliases", size = "xs", color = "text-muted" })
    table.insert(out, list(hi))
  end
  return column(out)
end

return {
  init = function() return { decision = "allow", res = "", rule_filter = "" } end,

  render = function(state, data)
    local sb = data.network_for
    current = sb
    local running = {}
    for _, s in ipairs(data.sandboxes or {}) do
      if s.status == "running" then table.insert(running, { id = s.name, label = s.name }) end
    end
    filter_now = data.network_view and data.network_view.filter or nil
    local body = { gap = 16, w = "full", requests(data, state) }
    table.insert(body, divider{})
    table.insert(body, row{ gap = 8, w = "full",
      text{ "Sandbox", size = "sm", color = "text-muted" },
      select{ id = "net-sandbox", items = running, value = sb, placeholder = "Choose a running sandbox", on = "pick" },
      spacer{},
      button{ icon = "reload", variant = "ghost", on = "refresh", help = "Read again", key = "net-refresh" },
    })
    if sb then
      local policy = data.network_policy
      if policy == nil then
        table.insert(body, note("Reading " .. sb .. "'s policy…"))
      else
        if policy.ok == false and policy.error then table.insert(body, note(policy.error, "red")) end
        local b = blocked_view(sb, policy)
        if b then table.insert(body, b) end
        table.insert(body, rules_view(sb, policy, state))
        local l = log_view(sb, policy, data.network_why)
        if l then table.insert(body, l) end
        local allow = policy.configured_allow or {}
        if #allow > 0 then
          table.insert(body, note("sbxw.toml allows on up: " .. table.concat(allow, ", ")))
        end
        table.insert(body, row{ gap = 8, w = "full", spacer{},
          button{ "Re-read policy now", icon = "reload", variant = "ghost", on = "policy_refresh", key = "policy-refresh", help = "sbx policy refresh" } })
      end
      table.insert(body, divider{})
      table.insert(body, ports_view(sb, data.network_ports, data.network_hosts, state, data.network_publish))
    else
      table.insert(body, note("Choose a running sandbox to see its network policy and ports."))
    end
    return column{ h = "full", w = "full", scroll{ gap = 0, column(body) } }
  end,

  actions = {
    answer = function(_, a) return intent("approval_answer", { id = a.id, option = a.option }) end,
    dismiss_all = function(_, a) return intent("approvals_dismiss", { sandbox = a.sandbox, expect = a.expect }) end,
    filter_clear = function() return intent("network_filter", {}) end,
    more = function(state) state.more = (state.more or 0) + 1 end,
    dismiss_group = function(_, a) return intent("approvals_dismiss", { sandbox = a.sandbox, title = a.title }) end,
    -- Picking another sandbox moves the filter with it, if one is on.
    pick = function(_, _, name) return intent("network_show", { sandbox = name, filter = filter_now ~= nil }) end,
    refresh = function() return intent("network_refresh") end,
    policy_refresh = function() return intent("policy_refresh") end,
    allow_blocked = function(_, host) return intent("network_allow_blocked", { sandbox = current, host = host }) end,
    rule_rm = function(_, id) return intent("network_rule_rm", { sandbox = current, rule = id }) end,
    unblock = function(_, id) return intent("network_unblock", { sandbox = current, rule = id }) end,
    add_rule = function(state)
      local res = state.res or ""
      if res == "" or not current then return nil end
      local args = { sandbox = current, resources = res, global = state.global or false }
      if state.http then
        if (state.methods or "") ~= "" then args.methods = state.methods end
        if (state.path or "") ~= "" then args.path = state.path end
        if (state.query or "") ~= "" then args.query = state.query end
      end
      state.res = ""
      return intent(state.decision == "deny" and "network_deny" or "network_allow", args)
    end,
    why = function(_, host) return intent("network_why", { sandbox = current, target = host }) end,
    why_close = function() return intent("network_why_close") end,
    port_open = function(_, spec) return intent("port_open", { sandbox = current, spec = spec }) end,
    port_rm = function(_, spec) return intent("port_unpublish", { sandbox = current, spec = spec }) end,
    publish = function(state)
      local sport = tonumber(state.sport or "")
      if not sport or not current then return nil end
      local hport = tonumber(state.hport or "")
      local args = { sandbox = current, sandbox_port = sport }
      if (state.alias or "") ~= "" then args.alias = state.alias end
      state.sport, state.hport = "", ""
      if hport then
        args.host_port = hport
        if (state.hip or "") ~= "" then args.host_ip = state.hip end
        return intent("port_publish_fixed", args)
      end
      return intent("port_publish", args)
    end,
    rule_filter = function(state, _, t) state.rule_filter = t or "" end,
    res = function(state, _, t) state.res = t or "" end,
    decision = function(state, _, d) state.decision = d end,
    global = function(state, _, on) state.global = on end,
    http = function(state, _, on) state.http = on end,
    methods = function(state, _, t) state.methods = t end,
    path = function(state, _, t) state.path = t end,
    query = function(state, _, t) state.query = t end,
    sport = function(state, _, t) state.sport = t end,
    hport = function(state, _, t) state.hport = t end,
    hip = function(state, _, t) state.hip = t end,
    alias = function(state, _, t) state.alias = t end,
  },
}
