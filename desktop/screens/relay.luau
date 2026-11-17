-- The Requests panel (docked on the right): what agents ask the person
-- through the relay (assets/js/relay.js): a question for another sandbox, a
-- screenshot, a round of questions, a virtual Mac request; and the
-- multiple-choice questions agents wait on (AskUserQuestion).
--
-- Everything shown here was written by an agent: it is drawn as text or
-- through `markdown`, never run. Routing and releasing return intents the
-- core confirms in its own dialog, from its own copy of what was typed and
-- attached; this screen only asks.

local TITLE = { question = "Information request", screenshot = "Screenshot request", ask = "Questions for you", mac = "Virtual Mac request" }
local KIND = { question = "question", screenshot = "screenshot", ask = "questions", mac = "virtual Mac" }

local function nz(s) return s ~= nil and s ~= "" end

local function settled(r) return r.state == "approved" or r.state == "denied" end

local function who(r)
  if nz(r.from_thread) then return r.from .. " · " .. r.from_thread end
  return r.from or "?"
end

local function gist(q)
  local line = string.match(q or "", "[^\n]*[^%s\n][^\n]*") or ""
  line = string.gsub(line, "^%s*❓%s*", "")
  if utf8.len(line) and utf8.len(line) > 60 then
    line = string.sub(line, 1, utf8.offset(line, 61) - 1) .. "…"
  end
  return line
end

local function section(title, count)
  return row{ gap = 8, w = "full",
    text{ title, size = "caption", weight = "semibold", color = "text-muted" },
    count and badge{ tostring(count), tone = "neutral" } or false,
  }
end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

-- The agent's words, framed as someone else's.
-- An agent's words, as written: keyword pills ([[Done]]) by the switch of
-- the sandbox that wrote them (`sb`), as text and pills only.
local function quote(t, sb)
  return column{ w = "full", pad = 8, radius = "sm", fill = "surface-alt", border = "border",
    text{ t or "", mono = true, size = "sm", color = "text", wrap = true, pills = sb or true } }
end

local function candidates(data, r)
  local out = {}
  for _, s in ipairs(data.sandboxes or {}) do
    if s.status == "running" and s.name ~= r.from and not s.unreachable_since then
      out[#out + 1] = s.name
    end
  end
  return out
end

local function targets(data, r, label)
  local names = candidates(data, r)
  if #names == 0 then
    return note("No other sandbox is running. Start one from the sidebar, or answer this yourself.")
  end
  local buttons = { gap = 8, w = "full" }
  for _, n in ipairs(names) do
    if n ~= r.to then
      buttons[#buttons + 1] = button{ n, icon = "send", on = "route", arg = { id = r.id, to = n }, key = "relay-to-" .. r.id .. "-" .. n,
        help = "Send it to " .. n .. "'s agent (the app shows you the text first)" }
    end
  end
  return column{ gap = 4, w = "full", text{ label, size = "sm", color = "text-muted" }, row(buttons) }
end

local function foot(r, send_label, can_send, extra)
  local kids = { gap = 8, w = "full",
    button{ "Later", variant = "ghost", on = "later", arg = r.id, key = "relay-later-" .. r.id, help = "Put it aside; it stays in the list" },
    r["local"] and button{ "Set aside", variant = "danger", on = "deny", arg = r.id, key = "relay-deny-" .. r.id, help = "Drop this form; nothing is typed into the session" }
      or button{ "Refuse", variant = "danger", on = "deny", arg = r.id, key = "relay-deny-" .. r.id, help = "Nothing is shared, and the agent is told not to ask again" },
  }
  if extra then kids[#kids + 1] = extra end
  kids[#kids + 1] = spacer{}
  if send_label then
    kids[#kids + 1] = button{ send_label, variant = "primary", icon = "send", on = "send", arg = r.id, disabled = not can_send, key = "relay-send-" .. r.id }
  end
  return row(kids)
end

local function from_line(r, why)
  return row{ gap = 8, w = "full", align = "start",
    badge{ who(r), tone = "blue" },
    text{ why, size = "sm", color = "text-muted", wrap = true, grow = 1 },
  }
end

-- A round of questions on one form.
local function round_form(r, ui, editable)
  local rd = (ui.rounds or {})[r.id] or { intro = "", questions = {} }
  local answers = (ui.forms or {})[r.id] or {}
  local listening = ui.voice and ui.voice.id == r.id and ui.voice.index or nil
  local kids = { gap = 12, w = "full" }
  if nz(rd.intro) then
    kids[#kids + 1] = column{ w = "full", pad = 8, radius = "sm", fill = "surface-alt", markdown{ rd.intro, size = "sm", color = "text-soft", pills = r.from or true } }
  end
  for i, q in ipairs(rd.questions or {}) do
    local qk = { gap = 4, w = "full", key = "relay-q-" .. r.id .. "-" .. i,
      border = listening == i and "blue" or nil, pad = listening == i and 6 or nil, radius = listening == i and "sm" or nil,
      row{ gap = 8, w = "full",
        text{ q.id, weight = "semibold", color = "text-bright", size = "sm" },
        nz(q.title) and text{ q.title, size = "sm", color = "text-bright", truncate = true, grow = 1 } or false,
      },
      markdown{ q.body, size = "sm", pills = r.from or true },
    }
    if nz(q.rec) then
      qk[#qk + 1] = row{ gap = 8, w = "full", align = "start",
        text{ "Recommended", size = "caption", weight = "semibold", color = "green" },
        text{ q.rec, size = "sm", color = "text-soft", wrap = true, grow = 1 },
      }
    end
    if editable then
      qk[#qk + 1] = row{ gap = 8, w = "full",
        column{ grow = 1, textarea{ id = "relay-ans-" .. r.id .. "-" .. i, value = answers[i] or "", placeholder = "Your answer, or empty to skip", rows = 2,
          on = "set", on_submit = "send_ask", arg = { id = r.id, field = "answer", index = i } } },
        nz(q.rec) and button{ "Use it", variant = "ghost", on = "use_rec", arg = { id = r.id, index = i }, key = "relay-rec-" .. r.id .. "-" .. i,
          disabled = (answers[i] or "") == q.rec, help = "Fill the answer with the recommendation" } or false,
      }
    end
    kids[#kids + 1] = column(qk)
  end
  return column(kids)
end

local function any_answer(ui, id)
  for _, a in ipairs((ui.forms or {})[id] or {}) do
    if nz(string.match(a, "%S")) then return true end
  end
  return false
end

local function shots_block(r, ui)
  local held = (ui.shots or {})[r.id] or {}
  local room = (ui.max_shots or 6) - #held
  local kids = { gap = 8, w = "full" }
  if #held > 0 then
    -- What is sent, in the order the agent receives it: each preview, numbered.
    local grid, n = { gap = 8, w = "full", align = "start" }, 0
    for _, s in ipairs(held) do
      grid[#grid + 1] = column{ gap = 2, w = 116, key = "relay-shot-" .. r.id .. "-" .. s.n,
        image{ key = s.image, max_w = 116, max_h = 80 },
        row{ gap = 4, w = "full",
          text{ s.n .. ". " .. s.w .. "×" .. s.h, size = "xs", color = "text-muted", truncate = true, grow = 1, help = s.label },
          button{ icon = "close", variant = "ghost", on = "shot_rm", arg = { id = r.id, index = s.n }, key = "relay-shot-rm-" .. r.id .. "-" .. s.n, help = "Remove this one" },
        },
      }
      n = n + 1
      if n % 3 == 0 then kids[#kids + 1] = row(grid); grid = { gap = 8, w = "full", align = "start" } end
    end
    if n % 3 ~= 0 then kids[#kids + 1] = row(grid) end
  end
  if room > 0 then
    kids[#kids + 1] = column{ w = "full", pad = #held == 0 and 14 or 8, radius = "sm", border = "border", fill = "surface-alt", align = "center", key = "relay-drop-" .. r.id,
      text{ #held == 0 and "Drop images here, or paste one" or ("Add another: drop or paste" .. (#held > 1 and (" · " .. #held .. " attached") or "")), size = "sm", color = "text-muted" } }
  else
    kids[#kids + 1] = note("That is " .. (ui.max_shots or 6) .. " images, the most one reply carries. Remove one to swap it for another.")
  end
  local acts = { gap = 8, w = "full" }
  if room > 0 then
    acts[#acts + 1] = button{ "Paste from clipboard", icon = "copy", on = "shot_paste", arg = r.id, key = "relay-shot-paste-" .. r.id }
  end
  if #held > 0 then
    acts[#acts + 1] = button{ "Remove all", variant = "ghost", on = "shot_rm", arg = { id = r.id }, key = "relay-shot-clear-" .. r.id }
  end
  if #acts > 2 then kids[#kids + 1] = row(acts) end
  return column(kids), #held
end

local function detail(data, ui, r)
  local kids = { gap = 12, w = "full", pad = 12, radius = "md", fill = "surface", border = settled(r) and "border" or "amber", key = "relay-card-" .. r.id,
    row{ gap = 8, w = "full",
      text{ TITLE[r.kind] or "Request", weight = "semibold", color = "text-bright", grow = 1 },
      text{ r.id, size = "xs", color = "text-dim", mono = true },
    },
  }
  local function add(x) if x then kids[#kids + 1] = x end end
  if nz(r.note) and not settled(r) then add(note(r.note, "amber")) end

  if settled(r) then
    add(from_line(r, KIND[r.kind] and ("asked for: " .. gist(r.question)) or ""))
    if r.state == "approved" then
      add(row{ gap = 8, icon{ "check", color = "green" }, text{ "Sent to " .. who(r) .. ".", color = "green", weight = "medium" } })
      if r.kind == "mac" and nz(r.answer) then add(quote(r.answer, r.to)) end
    else
      add(row{ gap = 8, icon{ "close", color = "red" }, text{ "Refused. Nothing was shared with " .. who(r) .. ".", color = "red", wrap = true, grow = 1 } })
    end
    add(row{ w = "full", spacer{}, button{ "Close", on = "later", arg = r.id, key = "relay-close-" .. r.id } })
    return column(kids)
  end

  if r.kind == "mac" then
    add(from_line(r, "asks you to approve this on the virtual Mac."))
    add(quote(r.question, r.from))
    if r.state == "routed" then
      add(note("Approved: sbxw is carrying it out. " .. r.from .. " gets the outcome."))
      return column(kids)
    end
    add(note("The virtual Mac is shared by every sandbox and has no personal or work account in it. Clicks and typing need no approval; this does."))
    add(foot(r, "Approve…", true))
    return column(kids)
  end

  if r.kind == "ask" then
    local n = #(((ui.rounds or {})[r.id] or {}).questions or {})
    local count = n == 1 and "one question" or (n .. " questions")
    if r.state == "routed" then
      add(from_line(r, "asks you " .. count .. "."))
      add(round_form(r, ui, false))
      add(row{ gap = 8, w = "full", icon{ "reload", color = "blue", size = 12 },
        text{ "Sent to " .. (r.to or "?") .. " for a draft. You review it before " .. r.from .. " receives anything.", size = "sm", color = "blue", wrap = true, grow = 1 } })
      add(targets(data, r, "Or ask another sandbox instead"))
      add(foot(r, nil, false))
      return column(kids)
    end
    add(from_line(r, (r["local"] and "asked you " .. count .. " in its session" or "asks you " .. count) .. ". Answer the ones you want; an empty answer skips its question."))
    local v = (data.jarvis and data.jarvis.voice) or {}
    if v.hears or v.speaks then
      local on = ui.voice and ui.voice.id == r.id
      add(row{ gap = 8, w = "full",
        v.hears and button{ on and "Stop answering by voice" or "Answer by voice", icon = "mic", variant = on and "primary" or "neutral",
          on = "voice", arg = { id = r.id, on = not on }, key = "relay-voice-" .. r.id,
          help = "Each question is read; what you say next (hold " .. (v.key or "F8") .. ") is its answer. Say repeat, skip or back." } or false,
        v.speaks and button{ "Read aloud", icon = "speaker", variant = "ghost", on = "read", arg = r.id, key = "relay-read-" .. r.id } or false,
      })
      if on then
        local q = ((((ui.rounds or {})[r.id] or {}).questions or {})[ui.voice.index or 1]) or {}
        add(note("Listening for " .. (q.id or "the next question") .. ": hold " .. (v.key or "F8") .. " and speak. Say repeat, skip or back.", "blue"))
      end
    end
    if r.state == "answered" then
      add(text{ (r.to or "?") .. " drafted these answers. Edit them, or clear them and answer yourself; " .. r.from .. " receives only what you send.", size = "sm", color = "text-muted", wrap = true })
      add(quote(r.answer, r.to))
    end
    add(round_form(r, ui, true))
    if r.state == "pending" and not r["local"] then add(targets(data, r, "Or ask one of these sandboxes to draft the answers")) end
    add(foot(r, "Send answers…", any_answer(ui, r.id),
      r.state == "answered" and button{ "Clear the draft", variant = "ghost", on = "clear", arg = r.id, key = "relay-clear-" .. r.id } or nil))
    return column(kids)
  end

  if r.kind == "screenshot" then
    add(from_line(r, "can't see its own screen, and asks, via you, for a look at what it just changed."))
    add(quote(r.question, r.from))
    local block, n = shots_block(r, ui)
    add(block)
    add(text{ "Or describe it instead", size = "caption", weight = "semibold", color = "text-muted" })
    local cap = (ui.caption or {})[r.id] or ""
    add(textarea{ id = "relay-caption-" .. r.id, value = cap, rows = 2, placeholder = "Words work too: say what it looks like", on = "set", arg = { id = r.id, field = "caption" } })
    add(foot(r, "Send to " .. r.from .. "…", n > 0 or nz(string.match(cap, "%S"))))
    return column(kids)
  end

  -- A question.
  add(from_line(r, "asks, via you, for information it can't see from its own workspace."))
  add(quote(r.question, r.from))
  if r.state == "pending" then
    add(targets(data, r, "Ask one of these sandboxes"))
    add(text{ "Or answer it yourself", size = "caption", weight = "semibold", color = "text-muted" })
    local own = (ui.own or {})[r.id] or ""
    add(textarea{ id = "relay-own-" .. r.id, value = own, rows = 3, placeholder = "Your answer goes to " .. r.from .. " once you confirm it", on = "set", arg = { id = r.id, field = "own" } })
    add(foot(r, "Send my answer…", nz(string.match(own, "%S"))))
  elseif r.state == "routed" then
    add(row{ gap = 8, w = "full", icon{ "reload", color = "blue", size = 12 },
      text{ "Sent to " .. (r.to or "?") .. ": waiting for its agent to answer. Nothing reaches " .. r.from .. " until you approve it.", size = "sm", color = "blue", wrap = true, grow = 1 } })
    add(targets(data, r, "Or ask someone else instead"))
    add(foot(r, nil, false))
  else
    add(text{ (r.to or "?") .. " answered. Edit it freely: this is what " .. r.from .. " will receive.", size = "sm", color = "text-muted", wrap = true })
    local review = (ui.review or {})[r.id] or ""
    add(textarea{ id = "relay-review-" .. r.id, value = review, rows = 8, on = "set", arg = { id = r.id, field = "review" } })
    add(foot(r, "Send to " .. r.from .. "…", nz(string.match(review, "%S"))))
  end
  return column(kids)
end

-- An agent waiting on a multiple-choice question.
local function ask_card(a)
  local label = a.thread and (a.sandbox .. " · " .. a.thread) or a.sandbox
  local base = "agent-" .. a.sandbox .. (a.thread and ("-" .. a.thread) or "")
  local kids = { gap = 8, w = "full", pad = 12, radius = "md", fill = "surface", border = "amber", key = base,
    row{ gap = 8, w = "full", badge{ label, tone = "blue" }, text{ "asks", size = "sm", color = "text-muted", grow = 1 } } }
  if not a.answerable or #(a.steps or {}) == 0 then
    local first = (a.steps or {})[1]
    kids[#kids + 1] = text{ (first and first.text) or a.activity or "Claude is waiting for you.", wrap = true, size = "sm", pills = a.sandbox }
    kids[#kids + 1] = row{ w = "full", spacer{}, button{ "Answer in its terminal", icon = "terminal", tone = "green", on = "terminal", arg = { sandbox = a.sandbox, thread = a.thread }, key = base .. "-terminal" } }
    return column(kids)
  end
  for si, step in ipairs(a.steps) do
    kids[#kids + 1] = text{ step.text or "", wrap = true, size = "sm", color = "text-bright", pills = a.sandbox }
    local opts = { gap = 8, w = "full" }
    for oi, o in ipairs(step.options or {}) do
      local picked = (a.picks or {})[si] == oi
      opts[#opts + 1] = button{ o, variant = picked and "primary" or "neutral", disabled = a.sending ~= nil,
        on = "pick", arg = { sandbox = a.sandbox, thread = a.thread, step = si, option = oi }, key = base .. "-" .. si .. "-" .. oi,
        help = (step.context or {})[oi] }
    end
    kids[#kids + 1] = column(opts)
  end
  if a.sending then
    kids[#kids + 1] = note("Sending: " .. a.sending)
  elseif #a.steps > 1 then
    kids[#kids + 1] = note("Several questions: the answer goes once each has a pick.")
  end
  return column(kids)
end

return {
  render = function(_, data)
    local ui = data.relay_ui or {}
    local reqs = {}
    for _, r in ipairs((data.relay and data.relay.requests) or {}) do reqs[#reqs + 1] = r end
    for _, r in ipairs(ui["local"] or {}) do reqs[#reqs + 1] = r end
    local closed = {}
    for _, id in ipairs(ui.closed or {}) do closed[id] = true end
    local waiting, out = {}, {}
    for _, r in ipairs(reqs) do
      if not settled(r) then
        if r.state == "routed" then out[#out + 1] = r else waiting[#waiting + 1] = r end
      end
    end
    table.sort(waiting, function(a, b) return (a.created_ms or 0) < (b.created_ms or 0) end)
    local current
    for _, r in ipairs(reqs) do
      if r.id == ui.current and not closed[r.id] then current = r end
    end
    current = current or waiting[1]
    local asks = {}
    -- Agents waiting on AskUserQuestion: sbxw's own sessions, not Jarvis's
    -- (its panel asks them), the newest per sandbox and thread.
    local by = {}
    for _, i in ipairs(data.sessions or {}) do
      local th = (i.thread ~= nil and i.thread ~= "") and i.thread or nil
      if i.state == "attention" and not (i.origin == "remote" and not th) and not (i.sandbox == "sbxw-jarvis" and not th) then
        local key = th and (i.sandbox .. "::" .. th) or i.sandbox
        local steps = i.steps
        if not steps or #steps == 0 then steps = i.question and { i.question } or {} end
        local a = { key = key, sandbox = i.sandbox, thread = th, answerable = i.answerable == true, activity = i.activity, steps = steps,
          picks = (ui.picks or {})[key] or {}, sending = (ui.sending or {})[key], ts = i.ts or 0 }
        if not by[key] then
          by[key] = a
          asks[#asks + 1] = a
        elseif (by[key].ts or 0) < a.ts then
          for n, x in ipairs(asks) do if x.key == key then asks[n] = a end end
          by[key] = a
        end
      end
    end

    local body = { gap = 12, w = "full" }
    local err = (data.app.errors or {}).relay
    if err then body[#body + 1] = note("Could not read the requests: " .. err, "red") end
    if #waiting > 1 or (#waiting == 1 and current and current.id ~= waiting[1].id) then
      local rows = {}
      for _, r in ipairs(waiting) do
        local on = current and current.id == r.id
        rows[#rows + 1] = row{ gap = 8, w = "full", on = "open", arg = r.id, key = "relay-req-" .. r.id, fill = on and "active-bg" or nil, radius = "sm", px = 4, py = 2,
          badge{ KIND[r.kind] or r.kind, tone = r.state == "answered" and "green" or "amber" },
          text{ who(r), size = "sm", weight = "medium", color = "text-bright" },
          text{ gist(r.question), size = "sm", color = "text-muted", truncate = true, grow = 1 },
        }
      end
      body[#body + 1] = column{ gap = 4, w = "full", section("Waiting for you", #waiting), list(rows) }
    end
    if current then body[#body + 1] = detail(data, ui, current) end
    local others = {}
    for _, r in ipairs(out) do
      if not current or r.id ~= current.id then
        others[#others + 1] = row{ gap = 8, w = "full", on = "open", arg = r.id, key = "relay-out-" .. r.id, radius = "sm", px = 4, py = 2,
          icon{ "reload", size = 12, color = "blue" },
          text{ (r.to or "?") .. " is answering", size = "sm", color = "text" },
          text{ "for " .. who(r) .. " · " .. gist(r.question), size = "sm", color = "text-muted", truncate = true, grow = 1 },
        }
      end
    end
    if #others > 0 then body[#body + 1] = column{ gap = 4, w = "full", section("With another sandbox", #others), list(others) } end
    if #asks > 0 then
      local cards = { gap = 8, w = "full", section("Agents asking", #asks) }
      for _, a in ipairs(asks) do cards[#cards + 1] = ask_card(a) end
      body[#body + 1] = column(cards)
    end
    if not current and #others == 0 and #asks == 0 then
      body[#body + 1] = note("Nothing waits for you. When an agent asks another sandbox something, asks for a screenshot or puts questions to you, it shows here.")
    end

    return column{ w = "full", h = "full", gap = 8,
      row{ w = "full", gap = 8,
        icon{ "chat", color = (#waiting + #asks) > 0 and "amber" or "text-muted" },
        text{ "Requests", weight = "semibold", color = "text-bright" },
        (#waiting + #asks) > 0 and badge{ tostring(#waiting + #asks), tone = "amber" } or false,
        spacer{},
      },
      scroll(body),
    }
  end,

  actions = {
    open = function(_, id) return intent("relay_open", { id = id }) end,
    later = function(_, id) return intent("relay_later", { id = id }) end,
    deny = function(_, id) return intent("relay_deny", { id = id }) end,
    route = function(_, a) return intent("relay_route", { id = a.id, to = a.to }) end,
    send = function(_, id) return intent("relay_send", { id = id }) end,
    send_ask = function(_, a) return intent("relay_send", { id = a.id }) end,
    clear = function(_, id) return intent("relay_clear", { id = id }) end,
    use_rec = function(_, a) return intent("relay_use_rec", { id = a.id, index = a.index }) end,
    set = function(_, a, text)
      local args = { id = a.id, field = a.field, index = a.index }
      if text ~= nil and text ~= "" then args.value = text end
      return intent("relay_set", args)
    end,
    shot_paste = function(_, id) return intent("relay_shot_paste", { id = id }) end,
    shot_choose = function(_, id) return intent("relay_shot_choose", { id = id }) end,
    shot_rm = function(_, a) return intent("relay_shot_remove", { id = a.id, index = a.index }) end,
    voice = function(_, a) return intent("relay_voice", { id = a.id, on = a.on }) end,
    read = function(_, id) return intent("relay_read", { id = id }) end,
    pick = function(_, a) return intent("agent_pick", { sandbox = a.sandbox, thread = a.thread, step = a.step, option = a.option }) end,
    terminal = function(_, a) return intent("open_terminal", { sandbox = a.sandbox, thread = a.thread, pane = "spare" }) end,
  },
}
