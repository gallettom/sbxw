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

-- CONCEPT 1, "Board": a card per session, grouped by sandbox; a "needs you"
-- card carries its question and its answers. Proposal only.

local function card(s, now, data)
  local need = s.state == "attention"
  local line = first_line(s.reply)
  local kids = { w = 272, gap = 8, pad = 12, radius = "md", fill = need and "amber-tint" or "surface", border = need and "amber" or "elevated",
    key = "mon-card-" .. (s.session_id or "?"),
    row{ gap = 8, w = "full", dot{ tone = TONE[s.state] }, text{ name_of(s), weight = "medium", color = "text-bright", truncate = true, grow = 1 },
      short_model(s) and text{ short_model(s), size = "sm", color = vendor_color(s) } or false } }
  if need and s.steps then
    local q = s.steps[1]
    kids[#kids + 1] = text{ q.text, wrap = true, color = "text-bright", pills = s.sandbox }
    local opts = { wrap = true, gap = 4, w = "full" }
    for i, o in ipairs(q.options) do opts[#opts + 1] = button{ o, tone = "amber", on = "pick_option", arg = i, dense = true } end
    kids[#kids + 1] = row(opts)
  elseif need then
    kids[#kids + 1] = text{ s.activity or "", wrap = true, color = "text-bright" }
    kids[#kids + 1] = row{ button{ "Open its terminal", icon = "terminal", tone = "green", on = "terminal", dense = true } }
  else
    kids[#kids + 1] = text{ s.activity or (line and "Last reply" or "Idle"), size = "sm", mono = s.state == "working", color = s.state == "working" and "blue" or "text-muted", truncate = true }
    kids[#kids + 1] = text{ line or s.last_input or "", size = "sm", color = "text", truncate = true, pills = s.sandbox }
  end
  kids[#kids + 1] = row{ gap = 8, w = "full",
    text{ ago(s.ts, now), size = "sm", color = "text-dim" },
    text{ money(s.cost_usd), size = "sm", color = "text-dim" },
    s.lines_added and text{ "+" .. s.lines_added .. " −" .. (s.lines_removed or 0), size = "sm", color = "text-dim", mono = true } or false }
  return column(kids)
end

return {
  render = function(state, data)
    local list, now = sessions(data), now_of(data)
    local by, order = {}, {}
    for _, s in ipairs(list) do
      if not by[s.sandbox] then by[s.sandbox] = {}; order[#order + 1] = s.sandbox end
      table.insert(by[s.sandbox], s)
    end
    local body = { gap = 16, px = 16, py = 16, w = "full" }
    body[#body + 1] = row{ gap = 8, w = "full",
      text{ #list .. " sessions in " .. #order .. " sandboxes", color = "text-muted" }, spacer{},
      count_needing(list) > 0 and badge{ count_needing(list) .. " need you", tone = "amber" } or false }
    for _, sb in ipairs(order) do
      local cpu = cpu_of(data, sb)
      local cards = { wrap = true, gap = 12, w = "full", align = "start" }
      for _, s in ipairs(by[sb]) do cards[#cards + 1] = card(s, now, data) end
      body[#body + 1] = column{ gap = 8, w = "full",
        row{ gap = 8, square(data, sb), text{ sb, weight = "semibold", color = "text-bright" },
          cpu and text{ "CPU " .. math.floor(cpu.cpu_pct) .. "%", size = "sm", color = "text-dim" } or false },
        row(cards) }
    end
    return scroll{ h = "full", column(body) }
  end,
}
