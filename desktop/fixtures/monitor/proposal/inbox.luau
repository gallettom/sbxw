-- Shared by the three concept screens (STAGE 1, throwaway): what the daemon
-- serves today, ordered by who needs the person first.

local RANK = { attention = 1, working = 2, idle = 3, exited = 4 }
local WORD = { attention = "needs you", working = "working", idle = "idle", exited = "ended" }
local TONE = { attention = "amber", working = "blue", idle = "neutral", exited = "neutral" }

local function sessions(data)
  local out = {}
  for _, s in ipairs(data.sessions or {}) do out[#out + 1] = s end
  table.sort(out, function(a, b)
    local ra, rb = RANK[a.state] or 9, RANK[b.state] or 9
    if ra ~= rb then return ra < rb end
    return (a.ts or 0) > (b.ts or 0)
  end)
  return out
end

local function now_of(data)
  local n = 0
  for _, s in ipairs(data.sessions or {}) do if (s.ts or 0) > n then n = s.ts end end
  return n + 60000
end

local function ago(ms, now)
  local m = math.floor(math.max(0, now - (ms or now)) / 60000)
  if m < 1 then return "now" end
  if m < 60 then return m .. " min" end
  return math.floor(m / 60) .. " h " .. (m % 60) .. " min"
end

local function took(ms)
  local m = math.floor((ms or 0) / 60000)
  if m < 60 then return m .. " min" end
  return math.floor(m / 60) .. " h " .. (m % 60)
end

local function name_of(s) return s.thread or "main" end
local function who(s) return s.sandbox .. " · " .. name_of(s) end
local function money(c) return c and string.format("$%.2f", c) or "no cost yet" end
local function first_line(t)
  if not t then return nil end
  for line in string.gmatch(t, "[^\n]+") do
    local l = string.gsub(line, "^%s+", "")
    if l ~= "" then return l end
  end
  return nil
end
local function permission(s) return s.state == "attention" and not s.steps end
local function short_model(s)
  if s.model_name then return s.model_name end
  if s.model then return (string.gsub(s.model, "^claude%-", "")) end
  return nil
end
local function cpu_of(data, sb)
  for _, x in ipairs((data.stats or {}).sandboxes or {}) do if x.name == sb then return x end end
  return nil
end
local function vendor_color(s) return s.model and "vendor-anthropic" or "text-muted" end
local function group_color(data, sb) return ((data.app or {}).groups or {})[sb] end
local function square(data, sb)
  local c = group_color(data, sb)
  return c and column{ w = 10, h = 10, radius = 2, fill = c } or column{ w = 10, h = 10, radius = 2, border = "text-dim" }
end
local function count_needing(list)
  local n = 0
  for _, s in ipairs(list) do if s.state == "attention" then n = n + 1 end end
  return n
end

-- CONCEPT 2, "Inbox and detail": left the sessions, who needs you first;
-- middle the selected one as a readable feed; bottom a composer; right its
-- facts and its links. Proposal only: no intent is wired.

local function list_row(s, sel, now)
  local sub = s.activity or first_line(s.reply) or s.last_input or ""
  return row{ key = "mon-row-" .. (s.session_id or "?"), on = "pick", arg = s.session_id, selected = sel,
    fill = sel and "active-bg" or nil, hover = "hover-subtle", radius = "sm", px = 8, py = 8, gap = 8, w = "full", align = "start",
    column{ py = 4, dot{ tone = TONE[s.state] } },
    column{ grow = 1, gap = 2,
      row{ gap = 4, w = "full",
        text{ s.sandbox, weight = "medium", color = "text-bright", truncate = true },
        text{ name_of(s), size = "sm", color = s.thread and "text-muted" or "text-dim", truncate = true, grow = 1 },
        text{ ago(s.ts, now), size = "sm", color = "text-dim" } },
      s.state == "attention" and text{ permission(s) and "Wants permission" or "Asks you a question", size = "sm", weight = "medium", color = "amber" } or false,
      text{ sub, size = "sm", color = "text-muted", truncate = true, mono = (s.state == "working"), pills = s.sandbox } } }
end

local function heading(t, n)
  return row{ px = 8, py = 4, gap = 4, text{ t, size = "sm", weight = "semibold", color = "text-muted" }, text{ tostring(n), size = "sm", color = "text-dim" } }
end

local function bubble(label, body, opts)
  opts = opts or {}
  return column{ gap = 4, w = "full",
    row{ gap = 4, icon{ opts.icon or "chat", size = 12, color = opts.ic or "text-muted" }, text{ label, size = "sm", weight = "medium", color = "text-muted" }, opts.right or false },
    body }
end

local function question_card(s)
  local q = (s.steps or {})[1]
  local kids = { gap = 8, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "mon-question",
    row{ gap = 8, icon{ "ask", color = "amber" }, text{ "Asks you", weight = "semibold", color = "amber" }, spacer{}, text{ "1 of " .. #(s.steps or {}), size = "sm", color = "text-muted" } },
    text{ q.text, wrap = true, color = "text-bright", pills = s.sandbox } }
  for i, o in ipairs(q.options) do
    local ctx = (q.context or {})[i]
    local desc = ctx and string.match(ctx, "— (.*)$") or nil
    kids[#kids + 1] = row{ gap = 8, w = "full", align = "start",
      button{ o, on = "pick_option", arg = i, tone = "amber", key = "mon-opt-" .. i },
      desc and text{ desc, size = "sm", wrap = true, color = "text-muted", grow = 1 } or false }
  end
  kids[#kids + 1] = text{ "A press answers it: the same path as the Requests panel's.", size = "sm", color = "text-dim" }
  return column(kids)
end

local function permission_card(s)
  return column{ gap = 8, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "mon-permission",
    row{ gap = 8, icon{ "warning", color = "amber" }, text{ "Wants your permission", weight = "semibold", color = "amber" } },
    text{ s.activity or "", wrap = true, color = "text-bright" },
    text{ "The daemon does not see which command it is asking about. Read it, and answer, in its terminal.", size = "sm", wrap = true, color = "text-muted" },
    row{ button{ "Open its terminal", icon = "terminal", tone = "green", on = "terminal", arg = s.session_id } } }
end

local function feed(s, now)
  local kids = { gap = 16, px = 24, py = 16, w = "full" }
  if s.last_input then
    kids[#kids + 1] = bubble("You", column{ pad = 12, radius = "md", fill = "elevated", w = "full", text{ s.last_input, wrap = true, color = "text-bright" } }, { icon = "prompt" })
  end
  if s.state == "working" then
    kids[#kids + 1] = row{ gap = 8, px = 12, py = 8, radius = "sm", border = "elevated", w = "full",
      icon{ "sparkle", size = 12, color = "blue" }, text{ "Now", size = "sm", weight = "medium", color = "blue" },
      text{ s.activity or "Working", mono = true, size = "sm", color = "text", truncate = true, grow = 1 }, text{ ago(s.ts, now), size = "sm", color = "text-dim" } }
  end
  if s.reply then
    kids[#kids + 1] = bubble("Claude", column{ w = "full", max_w = 720, markdown{ s.reply, pills = s.sandbox } }, { icon = "sparkle", ic = "vendor-anthropic",
      right = text{ "· last reply, " .. ago(s.ts, now) .. " ago", size = "sm", color = "text-dim" } })
  end
  if s.state == "attention" then
    kids[#kids + 1] = (s.steps and question_card or permission_card)(s)
  end
  if s.state == "idle" and not s.reply then
    kids[#kids + 1] = text{ "Idle. Nothing to read yet.", color = "text-muted" }
  end
  return scroll{ grow = 1, fade = 12, column(kids) }
end

local function composer(s, data)
  local waiting = s.state == "attention"
  local can = not waiting and (s.answerable or s.thread)
  local why = waiting and "Answer the card above first: typed text would land in its menu." or (not can and "sbxw cannot tell which terminal is this session's: answer in its terminal.") or nil
  return column{ gap = 8, px = 24, py = 12, w = "full",
    textarea{ id = "mon-compose", rows = 2, disabled = not can, placeholder = waiting and "Answer the card above first" or "Send a prompt to this session…", on = "typed" },
    row{ gap = 8, w = "full",
      icon{ "send", size = 12, color = "text-dim" },
      text{ "To", size = "sm", color = "text-dim" }, square(data, s.sandbox),
      text{ who(s), size = "sm", weight = "medium", color = "text", truncate = true, grow = 1 },
      can and text{ "⌘↩", size = "sm", color = "text-dim" } or false,
      button{ "Send…", icon = "send", tone = "green", on = "send", disabled = not can, help = "Opens the core's dialog with the text and the session it goes to" } },
    why and text{ why, size = "sm", color = "text-dim", wrap = true } or false }
end

local function fact(k, v, mono)
  return row{ gap = 8, w = "full", text{ k, size = "sm", color = "text-dim", w = 78 }, text{ v, size = "sm", color = "text", mono = mono, truncate = true, grow = 1 } }
end

local function facts(s, data)
  local cpu = cpu_of(data, s.sandbox)
  local wt = s.cwd and string.match(s.cwd, "%.sbxw%-worktrees/(.+)$")
  return column{ w = 230, gap = 8, px = 16, py = 16, fill = "surface-alt", h = "full",
    text{ "Session", size = "sm", weight = "semibold", color = "text-muted" },
    fact("Sandbox", s.sandbox), fact("Session", name_of(s)),
    fact("Worktree", wt and (".sbxw-worktrees/" .. wt) or "the workspace", true),
    fact("Branch", s.thread and ("sbxw/" .. s.thread) or "main checkout", true),
    fact("Model", short_model(s) or "not reported"),
    fact("Changed", (s.lines_added and ("+" .. s.lines_added .. " −" .. (s.lines_removed or 0)) or "–") .. " lines"),
    fact("Cost", money(s.cost_usd)), fact("Time", took(s.duration_ms)),
    fact("CPU", cpu and (math.floor(cpu.cpu_pct) .. "% (sandbox)") or "–"),
    divider{},
    text{ "Open", size = "sm", weight = "semibold", color = "text-muted" },
    button{ "Terminal", icon = "terminal", tone = "green", on = "terminal", arg = s.session_id, w = "full" },
    button{ "Commits", icon = "history", tone = "blue", on = "commits", arg = s.session_id, w = "full" },
    button{ "Files", icon = "folder", tone = "blue", on = "files", arg = s.session_id, w = "full" },
    button{ "Worktrees", icon = "branch", tone = "blue", on = "worktrees", arg = s.session_id, w = "full" } }
end

return {
  init = function() return {} end,
  render = function(state, data)
    local list, now = sessions(data), now_of(data)
    local sel = list[1]
    for _, s in ipairs(list) do if s.session_id == state.sel then sel = s end end
    local wide = ((data.app or {}).overlay or {}).w or 960
    local narrow = wide < 760
    local left = { gap = 0, w = "full", h = "full" }
    left[#left + 1] = row{ px = 12, py = 8, gap = 8, w = "full",
      text{ "Sessions", weight = "semibold", color = "text-bright" }, spacer{},
      count_needing(list) > 0 and badge{ count_needing(list) .. " need you", tone = "amber" } or false }
    local rows = { gap = 2, px = 4, py = 4, w = "full" }
    local last
    for _, s in ipairs(list) do
      if s.state ~= last then
        local n = 0
        for _, x in ipairs(list) do if x.state == s.state then n = n + 1 end end
        rows[#rows + 1] = heading(s.state == "attention" and "Needs you" or s.state == "working" and "Working" or "Idle", n)
        last = s.state
      end
      rows[#rows + 1] = list_row(s, s == sel, now)
    end
    left[#left + 1] = scroll{ grow = 1, column(rows) }
    if narrow then
      return column{ h = "full", w = "full", scroll{ grow = 1, column(rows) } }
    end
    local cpu = cpu_of(data, sel.sandbox)
    local wt = sel.cwd and string.match(sel.cwd, "%.sbxw%-worktrees/(.+)$")
    local strip = {}
    strip[#strip + 1] = wt and ("worktree " .. wt) or "main checkout"
    if sel.lines_added then strip[#strip + 1] = "+" .. sel.lines_added .. " −" .. (sel.lines_removed or 0) .. " lines" end
    strip[#strip + 1] = took(sel.duration_ms)
    if cpu then strip[#strip + 1] = "CPU " .. math.floor(cpu.cpu_pct) .. "%" end
    local head = column{ px = 24, py = 8, gap = 4, w = "full",
      row{ gap = 8, w = "full",
        dot{ tone = TONE[sel.state] }, square(data, sel.sandbox),
        text{ sel.sandbox, size = "title", weight = "semibold", color = "text-bright" },
        text{ name_of(sel), color = "text-muted" },
        badge{ WORD[sel.state], tone = TONE[sel.state] },
        spacer{},
        short_model(sel) and text{ short_model(sel), size = "sm", color = vendor_color(sel) } or false,
        text{ money(sel.cost_usd), size = "sm", color = "text-muted" } },
      row{ gap = 4, w = "full",
        text{ table.concat(strip, "  ·  "), size = "sm", color = "text-dim", truncate = true, grow = 1 },
        button{ icon = "terminal", tone = "green", variant = "ghost", dense = true, on = "terminal", help = "Open its terminal (the view closes)" },
        button{ icon = "history", tone = "blue", variant = "ghost", dense = true, on = "commits", help = "Its commits" },
        button{ icon = "folder", tone = "blue", variant = "ghost", dense = true, on = "files", help = "Its files" },
        button{ icon = "branch", tone = "blue", variant = "ghost", dense = true, on = "worktrees", help = "Its worktree" } } }
    local middle = column{ h = "full", grow = 1, head, divider{}, feed(sel, now), divider{}, composer(sel, data) }
    local right = wide >= 1180 and row{ h = "full", w = "full", middle, divider{ vertical = true }, facts(sel, data) } or row{ h = "full", w = "full", middle }
    return split{ id = "mon-split", vertical = true, ratio = wide >= 1100 and 0.27 or 0.36, column(left), right }
  end,
  actions = {
    pick = function(state, arg) state.sel = arg end,
  },
}
