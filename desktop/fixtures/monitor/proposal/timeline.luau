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

-- CONCEPT 3, "Timeline": one lane per session over the last hour. The
-- daemon keeps no history: a lane can only say "started here, in this state
-- since its last report". Proposal only.

local LABEL, LANE, SPAN = 220, 34, 3600000

return {
  render = function(state, data)
    local list, now = sessions(data), now_of(data)
    local wide = ((data.app or {}).overlay or {}).w or 960
    local room = math.max(200, wide - LABEL - 60)
    local t0 = now - SPAN
    local function X(t) return math.floor((math.max(t, t0) - t0) / SPAN * room) end
    local strokes, dots, kids = {}, {}, {}
    for i, s in ipairs(list) do
      local y = (i - 1) * LANE + LANE / 2
      strokes[#strokes + 1] = { points = { 0, y, room, y }, width = 1, color = "elevated" }
      local start = s.started_ms or t0
      strokes[#strokes + 1] = { points = { X(start), y, X(s.ts), y }, width = 6, color = "text-dim" }
      local tone = s.state == "attention" and "amber" or s.state == "working" and "blue" or "green"
      strokes[#strokes + 1] = { points = { X(s.ts), y, room, y }, width = 6, color = s.state == "idle" and "text-dim" or tone }
      dots[#dots + 1] = { x = X(s.ts), y = y, r = 5, color = tone }
    end
    local labels = { gap = 0, w = LABEL }
    for _, s in ipairs(list) do
      labels[#labels + 1] = row{ h = LANE, gap = 8, w = "full", dot{ tone = TONE[s.state] },
        text{ s.sandbox, size = "sm", weight = "medium", color = "text-bright", truncate = true },
        text{ name_of(s), size = "sm", color = "text-muted", truncate = true, grow = 1 } }
    end
    local ticks = { w = room, gap = 0, justify = "between" }
    for _, t in ipairs({ "60 min ago", "45", "30", "15", "now" }) do ticks[#ticks + 1] = text{ t, size = "sm", color = "text-dim" } end
    local sel = list[1]
    return column{ h = "full", w = "full", gap = 12, px = 16, py = 16,
      row{ gap = 8, text{ "The last hour", color = "text-muted" }, spacer{}, badge{ "needs you", tone = "amber" }, badge{ "working", tone = "blue" }, badge{ "idle", tone = "neutral" } },
      row{ align = "start", gap = 12, column(labels),
        column{ gap = 4, column{ w = room, h = #list * LANE, paint = { strokes = strokes, dots = dots } }, row(ticks) } },
      divider{},
      text{ "Grey: what the daemon cannot tell (it keeps the last state and its time, no history).", size = "sm", color = "text-dim", wrap = true },
      row{ gap = 8, w = "full", dot{ tone = "amber" }, text{ who(sel), weight = "medium", color = "text-bright" }, text{ (sel.steps and sel.steps[1].text) or sel.activity or "", color = "text", truncate = true, grow = 1 } } }
  end,
}
