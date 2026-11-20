-- Sessions (the feature `monitor`): every Claude session in one view, who
-- needs you first (left), the selected one's conversation with its question
-- and a composer (middle). A base view of the pane area. Built by
-- src/feat/monitor/: `data.monitor` is its view, kept from the daemon's
-- `session` events one card at a time.
--
-- Agent text (prompts, replies, questions, options) is data: drawn as text
-- and Markdown with keyword pills, never as a control. An option's press
-- sends its index; typed words go through the core's dialog.

local TONE = { you = "amber", working = "blue", idle = "neutral", ended = "neutral" }

-- Rows already built, by key, with what they were built from: an event about
-- one session builds that one row again, not the list (`rev` moves only
-- when its report changed).
local memo = {}
local built = 0

local function ago(ms, now)
  local m = math.floor(math.max(0, (now or 0) - (ms or now or 0)) / 60000)
  if m < 1 then return "now" end
  if m < 60 then return m .. " min" end
  if m < 1440 then return math.floor(m / 60) .. " h" end
  return math.floor(m / 1440) .. " d"
end

local function square(data, sb)
  local c = ((data.app or {}).groups or {})[sb]
  return c and column{ w = 8, h = 8, radius = 2, fill = c } or column{ w = 8, h = 8, radius = 2, border = "text-dim" }
end

-- One line a row: the status dot, the names, a chip and the time. Nested
-- under its leader (Lead Mode), a thread row names the thread alone. Each
-- name clips on its own (the thread keeps about 8 characters); the whole
-- activity line is the row's tooltip, and why a session needs the person
-- stays on the row, under it.
-- The list's width beside the detail (the names, not the activity).
local LIST_W = 280

local function list_row(r, sel, now, color)
  local when = ago(r.ts, now)
  local sig = tostring(r.rev) .. "|" .. when .. "|" .. tostring(sel) .. "|" .. tostring(color) .. "|" .. tostring(r.indent)
  local m = memo[r.key]
  if m and m.sig == sig then return m.el end
  built = built + 1
  local nested = r.indent
  local el = row{ key = "monitor-row-" .. r.key, on = "pick", arg = r.key, selected = sel, help = r.help,
    fill = sel and "active-bg" or nil, hover = "hover-subtle", radius = "sm", px = 8, py = 4, gap = 8, w = "full", align = "start",
    -- Team leads on: a leader's thread, one step in under its leader.
    nested and column{ w = 16, h = 1 } or false,
    column{ py = 4, dot{ tone = r.tone } },
    column{ grow = 1, gap = 2, min_w = 0,
      row{ gap = 4, w = "full", min_w = 0,
        (not nested and color) and column{ py = 4, column{ w = 8, h = 8, radius = 2, fill = color } } or false,
        (not nested) and text{ r.sandbox, key = "monitor-row-sb-" .. r.key, weight = "medium", color = "text-bright", truncate = true, min_w = r.chip and 56 or 72, max_w = 150, help = r.sandbox } or false,
        text{ r.name, key = "monitor-row-name-" .. r.key, size = "sm", color = nested and "text-bright" or (r.thread and "text-muted" or "text-dim"),
          weight = nested and "medium" or nil, truncate = true, grow = 1, min_w = r.chip and 48 or 56, help = r.name },
        r.chip and text{ r.chip, size = "sm", pills = r.sandbox, key = "monitor-row-chip-" .. r.key } or false,
        text{ when, size = "sm", color = "text-dim", key = "monitor-row-when-" .. r.key } },
      r.ask and text{ r.ask, size = "sm", weight = "medium", color = "amber", truncate = true } or false } }
  memo[r.key] = { sig = sig, el = el }
  return el
end

local function heading(t, n)
  return row{ px = 8, py = 4, gap = 4, text{ t, size = "sm", weight = "semibold", color = "text-muted" }, text{ tostring(n), size = "sm", color = "text-dim" } }
end

-- A message's label row over its body. The callers tint the body: `bubble-you`
-- (blue) for the person, `bubble-agent` (green) for Claude and any other agent,
-- each with its `-edge` border (theme.rs). `mine` puts the person's message on
-- the right (label and bubble hugging that edge), agents stay on the left.
local GUTTER = 40
local MAX_W = 720

local function bubble(label, mark, ic, right, body, mine)
  local head = row{ gap = 4, w = "full", justify = mine and "end" or nil, icon{ mark, size = 12, color = ic }, text{ label, size = "sm", weight = "medium", color = "text-muted" }, right or false }
  if not mine then
    return row{ gap = 0, w = "full",
      column{ gap = 4, grow = 1, head, body },
      column{ w = GUTTER, h = 1 } }
  end
  -- The person's side: a gutter on the left keeps the two sides apart at any width.
  return row{ gap = 0, w = "full", justify = "end",
    column{ w = GUTTER, h = 1 },
    column{ gap = 4, align = "end", grow = 1, max_w = MAX_W, head, body } }
end

local function question_card(d, data)
  local q = d.question
  local picks = (((data.relay_ui or {}).picks or {})[d.session]) or {}
  local sending = ((data.relay_ui or {}).sending or {})[d.session]
  local kids = { gap = 12, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "monitor-question",
    row{ gap = 8, w = "full", icon{ "ask", color = "amber" }, text{ "Asks you", weight = "semibold", color = "amber" }, spacer{},
      #q.steps > 1 and text{ #q.steps .. " questions", size = "sm", color = "text-muted" } or false } }
  for si, st in ipairs(q.steps) do
    local qk = { gap = 8, w = "full", text{ st.text, wrap = true, color = "text-bright", pills = d.sandbox } }
    for i, o in ipairs(st.options) do
      local picked = picks[si] == i
      qk[#qk + 1] = row{ gap = 8, w = "full", align = "start",
        q.answerable and button{ o.label, key = "monitor-opt-" .. si .. "-" .. i, on = "choose", arg = { key = d.key, step = si, option = i },
          tone = "amber", variant = picked and "primary" or nil, icon = picked and "check" or nil, disabled = sending ~= nil,
          help = "Answers with this option, the Requests panel's way" }
          or text{ i .. ". " .. o.label, color = "text", wrap = true },
        o.desc and text{ o.desc, size = "sm", wrap = true, color = "text-muted", grow = 1 } or false }
    end
    kids[#kids + 1] = column(qk)
  end
  if sending then
    kids[#kids + 1] = text{ "Sending: " .. sending, size = "sm", color = "text-muted", wrap = true }
  elseif q.answerable then
    kids[#kids + 1] = text{ #q.steps > 1 and "It is sent once each question has a pick." or "A press answers it.", size = "sm", color = "text-dim" }
  else
    kids[#kids + 1] = row{ gap = 8, w = "full",
      text{ "sbxw cannot answer this session from here.", size = "sm", color = "text-muted", grow = 1, wrap = true },
      button{ "Answer in its terminal", key = "monitor-answer-terminal", icon = "terminal", tone = "green", on = "link", arg = { name = "monitor_terminal", key = d.key } } }
  end
  return column(kids)
end

local function permission_card(d)
  return column{ gap = 8, pad = 12, radius = "md", fill = "amber-tint", border = "amber", w = "full", key = "monitor-permission",
    row{ gap = 8, icon{ "warning", color = "amber" }, text{ "Wants your permission", weight = "semibold", color = "amber" } },
    text{ d.permission, wrap = true, color = "text-bright" },
    text{ "sbxw does not see the command it asks about. Read it, and answer, in its terminal.", size = "sm", wrap = true, color = "text-muted" },
    row{ button{ "Open its terminal", key = "monitor-permission-terminal", icon = "terminal", tone = "green", on = "link", arg = { name = "monitor_terminal", key = d.key } } } }
end

-- The summary of a session the daemon has no conversation for (yet): the
-- last prompt, what it does now, its last reply.
local function summary(d, now, kids)
  if d.last_input then
    kids[#kids + 1] = bubble("You", "prompt", "text-muted", nil,
      column{ pad = 12, radius = "md", fill = "bubble-you", border = "bubble-you-edge", max_w = MAX_W, text{ d.last_input, wrap = true, color = "text-bright" } }, true)
  end
  if d.from then
    -- Another session's message (or the harness's): its words as data, quietly; never "You".
    kids[#kids + 1] = bubble("From " .. d.from.name, "people", "text-dim", nil,
      column{ pad = 12, radius = "md", fill = "bubble-agent", border = "bubble-agent-edge", w = "full", key = "monitor-from",
        text{ d.from.text ~= "" and d.from.text or "A message from another session.", wrap = true, size = "sm", color = "text-muted" } })
  end
  if d.reply then
    kids[#kids + 1] = bubble("Claude", "sparkle", "vendor-anthropic", text{ "· last reply, " .. ago(d.ts, now), size = "sm", color = "text-dim" },
      column{ pad = 12, radius = "md", fill = "bubble-agent", border = "bubble-agent-edge", w = "full", max_w = 720, key = "monitor-reply", markdown{ d.reply, pills = d.sandbox } })
  end
end

-- The conversation's rows, built once per row (by session, id and what it
-- shows): an event about one step builds that row again, not the 400.
local cmemo = {}
local cbuilt = 0

local STATUS = { ok = { "check", "green", "done" }, error = { "warning", "red", "failed" }, running = { "sparkle", "blue", "running" } }

local function quiet_line(key, ic, t)
  return row{ key = key, gap = 8, w = "full", justify = "center", py = 4,
    icon{ ic, size = 12, color = "text-dim" }, text{ t, size = "sm", color = "text-muted", wrap = true, max_w = 560 } }
end

-- One step: its tool's icon, the one-line summary, the result's size, a
-- status mark; pressed (when it has a result), the result's head below.
local function step_row(st, skey, nested)
  local m = STATUS[st.status] or STATUS.ok
  local head = row{ gap = 8, w = "full", px = 8, py = 4, radius = "sm", hover = st.has_head and "hover-subtle" or nil,
    key = "monitor-step-" .. st.id, on = st.has_head and "expand" or nil, arg = st.has_head and { key = skey, id = st.id } or nil,
    help = st.has_head and (st.open and "Hide its output" or "Show the start of its output") or nil,
    nested and column{ w = 16, h = 1 } or false,
    icon{ st.icon, size = 12, color = "text-muted" },
    text{ st.summary, size = "sm", mono = true, color = "text", truncate = true, grow = 1 },
    st.size and text{ st.size, size = "sm", color = "text-dim" } or false,
    icon{ m[1], size = 12, color = m[2], help = m[3] } }
  if not (st.open and st.head) then return head end
  return column{ gap = 4, w = "full", head,
    column{ key = "monitor-out-" .. st.id, px = 12, py = 8, radius = "sm", fill = "floor", w = "full",
      text{ st.head, mono = true, size = "sm", color = "text-muted", wrap = true } } }
end

local function conv_row(r, d, now)
  if r.t == "day" then
    return row{ gap = 8, w = "full", py = 4, key = "monitor-" .. r.id, column{ grow = 1, h = 1, fill = "elevated" }, text{ r.label, size = "xs", color = "text-muted" }, column{ grow = 1, h = 1, fill = "elevated" } }
  elseif r.t == "you" then
    return column{ key = "monitor-you-" .. r.id, w = "full",
      bubble("You", "prompt", "text-muted", text{ "· " .. ago(r.at, now), size = "sm", color = "text-dim" },
        column{ pad = 12, radius = "md", fill = "bubble-you", border = "bubble-you-edge", max_w = MAX_W, key = "monitor-body-you-" .. r.id, text{ r.text, wrap = true, color = "text-bright" } }, true) }
  elseif r.t == "from" then
    -- Typed in by another session or the harness: quiet, never "You".
    local kids = { pad = 12, gap = 4, radius = "md", fill = "bubble-agent", border = "bubble-agent-edge", w = "full",
      text{ r.text ~= "" and r.text or "A message from another session.", wrap = true, size = "sm", color = "text-muted" } }
    if r.long then
      kids[#kids + 1] = row{ button{ r.open and "Show less" or "Show all", key = "monitor-fold-" .. r.id, variant = "ghost", dense = true, on = "expand", arg = { key = d.key, id = r.id } } }
    end
    return column{ key = "monitor-from-" .. r.id, w = "full",
      bubble("From " .. r.name, "people", "text-dim", text{ "· " .. ago(r.at, now), size = "sm", color = "text-dim" }, column(kids)) }
  elseif r.t == "claude" then
    return column{ key = "monitor-claude-" .. r.id, w = "full",
      bubble("Claude", "sparkle", "vendor-anthropic", text{ "· " .. ago(r.at, now), size = "sm", color = "text-dim" },
        column{ pad = 12, radius = "md", fill = "bubble-agent", border = "bubble-agent-edge", w = "full", max_w = MAX_W, key = "monitor-body-claude-" .. r.id, markdown{ r.text, pills = d.sandbox } }) }
  elseif r.t == "step" then
    return step_row(r, d.key)
  elseif r.t == "group" then
    local m = STATUS[r.status] or STATUS.ok
    local label = r.n .. " steps" .. ((r.errors or 0) > 0 and (" · " .. r.errors .. " failed") or "")
    local kids = { gap = 2, w = "full", key = "monitor-group-" .. r.id,
      row{ gap = 8, w = "full", px = 8, py = 4, radius = "sm", hover = "hover-subtle", key = "monitor-steps-" .. r.id, on = "expand", arg = { key = d.key, id = r.id },
        help = r.open and "Fold the steps" or "Show each step",
        icon{ r.open and "chevron-down" or "chevron-right", size = 12, color = "text-muted" },
        text{ label, size = "sm", weight = "medium", color = "text" },
        icon{ r.icon, size = 12, color = "text-dim" },
        text{ r.summary, size = "sm", mono = true, color = "text-muted", truncate = true, grow = 1 },
        icon{ m[1], size = 12, color = m[2], help = m[3] } } }
    if r.open then
      for _, st in ipairs(r.steps or {}) do kids[#kids + 1] = step_row(st, d.key, true) end
    end
    return column(kids)
  elseif r.t == "system" then
    return quiet_line("monitor-sys-" .. r.id, r.icon or "info", r.text)
  end
  return false
end

-- `stick`: the conversation follows its end (it opens there). Once a fold
-- is pressed it stops following until the next selection: opening a block
-- near the end would otherwise slide everything above it up.
local function feed(d, now, data, stick)
  local kids = { gap = 12, px = 24, py = 16, w = "full" }
  local c = d.conv
  local seen = {}
  if c and (c.state == "ready" or c.state == "missing") and c.rows then
    if c.earlier then
      kids[#kids + 1] = row{ w = "full", justify = "center",
        button{ c.older_loading and "Reading…" or "Show earlier", key = "monitor-earlier", icon = "history", variant = "ghost", dense = true,
          on = "earlier", arg = { key = d.key }, disabled = c.older_loading, help = "The earlier part of the conversation" } }
    elseif c.truncated then
      kids[#kids + 1] = quiet_line("monitor-truncated", "info", "Earlier conversation is not kept")
    end
    if c.continues and not c.earlier then
      kids[#kids + 1] = quiet_line("monitor-continues", "history", "This session continues an earlier one, not shown here")
    end
    for _, r in ipairs(c.rows) do
      local mk = d.key .. "|" .. r.id
      local sig = r.sig .. "|" .. (r.at and ago(r.at, now) or "")
      local m = cmemo[mk]
      if not (m and m.sig == sig) then
        cbuilt = cbuilt + 1
        m = { sig = sig, el = conv_row(r, d, now) }
        cmemo[mk] = m
      end
      seen[mk] = true
      if m.el then kids[#kids + 1] = m.el end
    end
    if #c.rows == 0 then
      kids[#kids + 1] = text{ "Nothing said yet in this session.", key = "monitor-conv-empty", color = "text-muted", wrap = true }
    end
  else
    summary(d, now, kids)
    local why = c and (c.state == "partial" and c.partial
      or c.state == "loading" and "Reading the conversation…"
      or c.state == "error" and ("The conversation could not be read: " .. tostring(c.error)))
      or "sbxw has no conversation for this session: its last prompt, what it does now and its last reply."
    kids[#kids + 1] = text{ why, key = "monitor-conv-note", size = "sm", color = "text-dim", wrap = true }
  end
  for k in pairs(cmemo) do if not seen[k] then cmemo[k] = nil end end
  if c and c.error and c.state ~= "error" then
    kids[#kids + 1] = row{ gap = 8, w = "full", key = "monitor-conv-error",
      text{ "The last read failed; what is shown may be behind.", size = "sm", color = "text-dim", wrap = true, grow = 1 },
      button{ "Try again", key = "monitor-retry", variant = "ghost", dense = true, on = "retry", arg = { key = d.key } } }
  elseif c and c.state == "error" then
    kids[#kids + 1] = row{ button{ "Try again", key = "monitor-retry", variant = "ghost", dense = true, on = "retry", arg = { key = d.key } } }
  end
  -- What it does now, pinned under the conversation while it works, then the cards.
  if d.now then
    kids[#kids + 1] = row{ gap = 8, px = 12, py = 8, radius = "sm", border = "elevated", w = "full", key = "monitor-now",
      icon{ "sparkle", size = 12, color = "blue" }, text{ "Now", size = "sm", weight = "medium", color = "blue" },
      text{ d.now, mono = d.now_mono ~= false, size = "sm", color = "text", truncate = true, grow = 1 }, text{ ago(d.ts, now), size = "sm", color = "text-dim" } }
  end
  if d.question then
    kids[#kids + 1] = question_card(d, data)
  elseif d.permission then
    kids[#kids + 1] = permission_card(d)
  end
  if #kids == 0 then
    kids[#kids + 1] = text{ "Nothing to read yet: the session has not reported a prompt or a reply.", color = "text-muted", wrap = true }
  end
  return scroll{ grow = 1, fade = 12, stick_bottom = stick ~= false, key = "monitor-conv-" .. d.key, column(kids) }
end

local function draft_of(state, d)
  local dr = (state.drafts or {})[d.key]
  -- A draft typed before the last send went with it.
  if dr and dr.gen == d.sent then return dr.text end
  return ""
end

-- The files attached to this session's next Send: name (cut by the core),
-- size, why it needs a look or cannot go, a remove cross.
local function chips(d)
  local files = d.files or {}
  if #files == 0 then return false end
  local kids = { gap = 8, w = "full", wrap = true, key = "monitor-attached" }
  for _, f in ipairs(files) do
    local tone = f.state == "refused" and "red" or f.state == "look" and "amber" or f.state == "uploaded" and "green" or "text-muted"
    local ic = f.state == "refused" and "close" or f.state == "look" and "shield" or f.state == "uploaded" and "check" or "file"
    kids[#kids + 1] = row{ gap = 4, px = 8, h = 24, radius = "sm", border = f.state == "ok" and "elevated" or tone, key = "monitor-file-" .. f.id, help = f.why,
      icon{ ic, size = 12, color = tone },
      text{ f.name, size = "sm", color = "text", truncate = true, max_w = 260 },
      text{ f.sending and string.format("%d%%", math.floor((f.progress or 0) * 100)) or f.size, size = "sm", color = "text-dim" },
      button{ icon = "close", key = "monitor-file-remove-" .. f.id, variant = "ghost", dense = true, on = "unattach", arg = { key = d.key, id = f.id },
        disabled = d.sending == true, help = "Remove it: it is not sent" } }
  end
  return row(kids)
end

local function composer(state, d, data)
  local c = d.compose
  local covered = (data.app or {}).covered == true
  local can = c.can and not covered and not d.sending
  local draft = draft_of(state, d)
  local files = d.files or {}
  local refused = false
  for _, f in ipairs(files) do if f.state == "refused" then refused = true end end
  local narrow = ((data.app or {}).overlay or {}).w or 1000
  -- Dictation (the core's, dictate.rs): what it does in this field now.
  local dict = (data.app or {}).dictation or {}
  local function here(x) return x and x.screen == "monitor" and x.input == "monitor-compose" end
  local rec = here(dict.recording) and dict.recording or nil
  local writing = here(dict.transcribing)
  local note = here(dict.notice) and dict.notice.text or nil
  local listening = false
  if rec then
    listening = row{ gap = 8, w = "full", key = "monitor-dictating",
      dot{ tone = "red" },
      text{ string.format("Listening %d:%02d", rec.secs // 60, rec.secs % 60), size = "sm", weight = "medium", color = "red-text" },
      column{ w = 80, progress{ value = rec.level, tone = "red" } },
      text{ rec.held and ("Release " .. dict.key .. " to put the words in the field. Escape cancels.") or "Press the microphone again to put the words in the field. Escape cancels.",
        size = "sm", color = "text-muted", truncate = true, grow = 1 } }
  elseif writing then
    listening = text{ "Writing down your words, masked first…", size = "sm", color = "text-muted", key = "monitor-dictating" }
  elseif note then
    listening = text{ note, size = "sm", color = "text-muted", wrap = true, key = "monitor-dictate-note" }
  end
  return column{ gap = 8, px = 24, py = 12, w = "full",
    textarea{ id = "monitor-compose", rows = 2, value = draft, disabled = not c.can, tone = "compose",
      placeholder = c.can and ("Write to " .. c.target .. "…") or "",
      on = "typed", on_submit = "submit", arg = { key = d.key, gen = d.sent } },
    listening,
    chips(d),
    row{ gap = 8, w = "full",
      icon{ "send", size = 12, color = "text-dim" },
      text{ "To", size = "sm", color = "text-dim" }, square(data, d.sandbox),
      text{ c.target, size = "sm", weight = "medium", color = "text", truncate = true, grow = 1, key = "monitor-target" },
      button{ narrow >= 640 and "Attach" or nil, key = "monitor-attach", icon = "plus", variant = "ghost", dense = true, on = "attach", arg = { key = d.key },
        disabled = not can or d.attach_max == true,
        help = "Attach a file to what you send (the system's file dialog); you can also drop files here or paste a picture" },
      button{ key = "monitor-dictate", icon = "mic", variant = rec and "danger" or "ghost", dense = true, on = "dictate",
        disabled = not rec and not can,
        keys = (not rec and can) and { dict.key or "F9" } or nil,
        help = rec and "Listening: press again to put the words in the field, Escape cancels"
          or (not c.can and c.why)
          or ("Dictate: press, speak, press again (or hold " .. (dict.key or "F9") .. "). The words are masked and put in the field; nothing is sent") },
      button{ "Send…", key = "monitor-send", icon = "send", tone = "green", on = "send", arg = { key = d.key, gen = d.sent },
        disabled = not can or refused or (draft == "" and #files == 0),
        help = "Opens the core's dialog with your words in full, the files and the session they go to", keys = { (data.app or {}).platform == "macos" and "⌘" or "Ctrl", "↩" } } },
    (not c.can) and text{ c.why, size = "sm", color = "text-dim", wrap = true } or false }
end

local function header(d, data)
  return column{ px = 24, py = 8, gap = 4, w = "full",
    row{ gap = 8, w = "full",
      dot{ tone = d.tone }, square(data, d.sandbox),
      text{ d.sandbox, size = "title", weight = "semibold", color = "text-bright", truncate = true },
      text{ d.name, color = "text-muted" },
      badge{ d.word, tone = d.tone },
      spacer{},
      d.model and text{ d.model, size = "sm", color = "vendor-anthropic" } or false,
      text{ d.cost or "–", size = "sm", color = "text-muted", help = d.cost_help } },
    row{ gap = 4, w = "full",
      text{ d.strip, size = "sm", color = "text-dim", truncate = true, grow = 1 },
      button{ icon = "terminal", key = "monitor-terminal", tone = "green", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_terminal", key = d.key },
        help = d.pane and ("Its terminal, in pane " .. d.pane .. " (the view closes)") or "Open its terminal (the view closes)" },
      button{ icon = "history", key = "monitor-commits", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_commits", key = d.key }, help = "Its commits" },
      button{ icon = "folder", key = "monitor-files", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_files", key = d.key }, help = "Its files" },
      button{ icon = "branch", key = "monitor-worktrees", tone = "blue", variant = "ghost", dense = true, on = "link", arg = { name = "monitor_worktrees", key = d.key }, help = "Its worktree" } } }
end

local function empty(view)
  return column{ key = "monitor-empty", gap = 8, pad = 24, w = "full", h = "full",
    text{ view and "No session yet" or "Loading the sessions", size = "title", weight = "semibold", color = "text-bright" },
    text{ view and "A Claude session that starts in a sandbox shows here: who needs you first, then who works." or "", color = "text-muted", wrap = true },
    (not view) and row{ button{ "Show the sessions", key = "monitor-load", on = "load", tone = "blue" } } or false }
end

local screen = {
  init = function() return { drafts = {} } end,
  render = function(state, data)
    local v = data.monitor
    local leads = v and (v.leads or {}).on == true
    if not v or (#(v.rows or {}) == 0 and not leads) then
      memo = {}
      return empty(v)
    end
    local now, counts, d = v.now, v.counts or {}, v.detail
    local groups = (data.app or {}).groups or {}
    local rows = { gap = 2, px = 4, py = 4, w = "full", group = "list", key = "monitor-list" }
    local last
    local seen = {}
    for _, r in ipairs(v.rows) do
      -- Team leads on: each leader with its team, no state headings.
      if not leads and r.group ~= last then
        local title = r.group == "you" and "Needs you" or r.group == "working" and "Working" or r.group == "idle" and "Idle" or "Ended"
        rows[#rows + 1] = heading(title, counts[r.group] or 0)
        last = r.group
      end
      rows[#rows + 1] = list_row(r, r.key == v.sel, now, groups[r.sandbox])
      seen[r.key] = true
    end
    for k in pairs(memo) do if not seen[k] then memo[k] = nil end end
    if leads and #v.rows == 0 then
      rows[#rows + 1] = column{ key = "monitor-leads-empty", gap = 4, px = 8, py = 12, w = "full",
        text{ "No team leader is running", weight = "medium", color = "text-bright" },
        text{ "A sandbox whose lead runs a plan shows here with its threads. Lead Mode Off, in the top bar, shows every session.", size = "sm", color = "text-muted", wrap = true } }
    end
    local more = more_rows(v.count, { key = "monitor-more", on = "more", what = "sessions" })
    if more then rows[#rows + 1] = more end
    local you = counts.you or 0
    local head_text = leads and "Team leaders and their threads" or (counts.total or 0) .. " session" .. ((counts.total or 0) == 1 and "" or "s")
    local top = row{ px = 12, py = 8, gap = 8, w = "full", min_w = 0,
      -- The view's title says "Sessions" already: the list's head is its
      -- count. It clips (whole in its tooltip); what needs you never does.
      text{ head_text, key = "monitor-count", size = "sm", weight = "medium", color = "text-muted", truncate = true, grow = 1, help = head_text },
      you > 0 and button{ you .. " need" .. (you == 1 and "s" or "") .. " you", key = "monitor-next", tone = "amber", dense = true, on = "next",
        help = "The next session that needs you", keys = { "N" } }
        or text{ "Nothing needs you", key = "monitor-calm", size = "sm", color = "text-dim" } }
    -- Wide (beside the detail): a fixed width, enough for a sandbox and a
    -- thread name (each clips on its own); the detail takes the rest.
    local side = overlay_wide(data) and d
    local left = column{ gap = 0, w = side and LIST_W or "full", h = "full", min_w = 0, key = "monitor-list-col", top, scroll{ grow = 1, column(rows) } }
    local middle = d and column{ h = "full", grow = 1, key = "monitor-detail",
      header(d, data), divider{}, feed(d, now, data, state.unstuck ~= d.key), divider{}, composer(state, d, data) }
    if not overlay_wide(data) or not d then
      -- Narrow: the list, or the session a press opened, with the way back.
      if d and state.open == d.key then
        return column{ h = "full", w = "full",
          row{ px = 12, py = 4, w = "full", button{ "All sessions", key = "monitor-back", icon = "threads", variant = "ghost", dense = true, on = "back" } },
          middle }
      end
      return column{ h = "full", w = "full", left }
    end
    return row{ h = "full", w = "full", gap = 0, align = "stretch", key = "monitor-columns",
      left, column{ w = 1, h = "full", fill = "border", key = "monitor-divider" }, column{ grow = 1, h = "full", min_w = 0, middle } }
  end,
  actions = {
    load = function() return intent("monitor_open", {}) end,
    pick = function(state, key)
      state.open = key
      state.unstuck = nil
      return intent("monitor_select", { key = key })
    end,
    back = function(state) state.open = nil end,
    next = function() return intent("monitor_next", {}) end,
    more = function() return intent("monitor_more", {}) end,
    dictate = function() return intent("dictate", { input = "monitor-compose" }) end,
    link = function(_, a) return intent(a.name, { key = a.key }) end,
    choose = function(_, a) return intent("monitor_pick", { key = a.key, step = a.step, option = a.option }) end,
    -- Typing sends nothing: the draft is kept, by session.
    typed = function(state, a, text)
      state.drafts = state.drafts or {}
      state.drafts[a.key] = { text = text or "", gen = a.gen }
    end,
    -- Words, files, or both: the core's Rust checks what may go.
    send = function(state, a)
      local dr = (state.drafts or {})[a.key]
      local text = (dr and dr.gen == a.gen) and dr.text or ""
      -- No words: no `text` (files alone; the Rust says when nothing would go).
      return intent("monitor_send", { key = a.key, text = string.find(text, "%S") and text or nil })
    end,
    submit = function(state, a, text)
      state.drafts = state.drafts or {}
      state.drafts[a.key] = { text = text or "", gen = a.gen }
      return intent("monitor_send", { key = a.key, text = string.find(text or "", "%S") and text or nil })
    end,
    attach = function(_, a) return intent("monitor_attach", { key = a.key }) end,
    expand = function(state, a)
      state.unstuck = a.key
      return intent("monitor_expand", { key = a.key, id = a.id })
    end,
    earlier = function(_, a) return intent("monitor_earlier", { key = a.key }) end,
    retry = function(_, a) return intent("monitor_retry", { key = a.key }) end,
    unattach = function(_, a) return intent("monitor_attach_remove", { key = a.key, id = a.id }) end,
  },
}

-- What the tests read: rows built since the screen loaded.
screen.built = function() return built end
screen.conv_built = function() return cbuilt end

return screen
