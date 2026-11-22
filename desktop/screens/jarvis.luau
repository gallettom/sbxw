-- Jarvis, docked on the right: a plain conversation (your words, Jarvis's
-- answers as Markdown), one input row with a compact talk button, and only
-- the log scrolls. The core draws the proposal dialog; this screen can only
-- open it (approve_proposal) and never approves anything.

local WHY = {
  unknown = "Asking the daemon about Jarvis…",
  absent = "Jarvis is sbxw's assistant: a Claude session in a sandbox of its own that can propose changes to sbxw's configuration. Nothing is applied until you approve it.",
  creating = "Creating Jarvis's sandbox. The first start takes a minute or two.",
  starting = "Starting Jarvis…",
  stopped = "Jarvis's sandbox is stopped.",
}

local function message(m, speaks)
  if m.who == "me" then
    return row{ w = "full", justify = "end",
      column{ max_w = 300, px = 12, py = 8, radius = "sm", fill = "elevated",
        text{ m.text, wrap = true, color = "text-bright" },
      },
    }
  elseif m.who == "jarvis" then
    return column{ w = "full", gap = 2,
      markdown{ m.text, on = "link", key = "msg-" .. m.id, pills = true },
      speaks and row{ w = "full", justify = "end",
        button{ icon = "speaker", variant = "ghost", on = "read", arg = m.id,
          color = m.reading and "blue" or "text-dim",
          help = m.reading and "Stop reading" or "Read aloud" },
      } or false,
    }
  elseif m.who == "agent" then
    -- An agent of the sandbox the panel follows: its words, framed as its.
    return column{ w = "full", gap = 4, key = "msg-" .. m.id,
      row{ gap = 8, w = "full", badge{ m.from or "agent", tone = "blue" }, spacer{},
        speaks and button{ icon = "speaker", variant = "ghost", on = "read", arg = m.id,
          color = m.reading and "blue" or "text-dim", help = m.reading and "Stop reading" or "Read aloud" } or false },
      markdown{ m.text, on = "link", pills = m.from or true },
    }
  elseif m.who == "error" then
    return text{ m.text, wrap = true, size = "sm", color = "red" }
  end
  return text{ m.text, wrap = true, size = "sm", color = "text-muted" }
end

-- Why the last message did not leave, beside the input. Hidden once the
-- person edits the input (the core clears it when a send succeeds or Jarvis
-- runs). The button says what it does: start and send the kept text.
local function send_note(j, state)
  local e = j.send
  if not e then return false end
  if not e.queued and state.edited ~= nil and state.edited ~= e.input then return false end
  local offer = false
  if e.offer == "start" then
    offer = button{ "Start Jarvis and send", icon = "play", variant = "primary", on = "start_send",
      help = "Starts Jarvis, then sends what is in the input" }
  elseif e.offer == "wait" then
    offer = button{ "Send once it runs", variant = "ghost", on = "start_send",
      help = "Sends what is in the input as soon as Jarvis runs" }
  end
  return row{ w = "full", gap = 8,
    column{ grow = 1, text{ e.text, wrap = true, size = "sm", color = e.queued and "text-muted" or "red" } },
    offer,
  }
end

return {
  render = function(state, data)
    local j = data.jarvis
    local v = j.voice
    local running = j.status == "running"

    -- Where the words go (feat/relay, follow): Jarvis, or a running sandbox's agent.
    local f = (data.relay_ui and data.relay_ui.follow) or {}
    local target = f.target
    local items = { { id = "sbxw-jarvis", label = "Jarvis", icon = "jarvis" } }
    local listed = false
    for _, sb in ipairs(data.sandboxes or {}) do
      if sb.status == "running" and not sb.unreachable_since and sb.name ~= "sbxw-jarvis" then
        items[#items + 1] = { id = sb.name, label = sb.name, icon = "terminal" }
        if sb.name == target then listed = true end
      end
    end
    if target and not listed then items[#items + 1] = { id = target, label = target .. " (stopped)", icon = "terminal" } end

    -- The state shown in the header, in the sidebar's words and dots: the
    -- picked sandbox's (its newest session waiting on the person, working,
    -- or the sandbox's status), else Jarvis's own.
    local st
    if target then
      local newest
      for _, i in ipairs(data.sessions or {}) do
        if i.sandbox == target and (newest == nil or (i.ts or 0) > (newest.ts or 0)) then newest = i end
      end
      local status = "gone"
      for _, sb in ipairs(data.sandboxes or {}) do if sb.name == target then status = sb.status end end
      if status == "running" and newest and newest.state == "attention" then st = { "wants you", "amber" }
      elseif status == "running" and newest and newest.state == "working" then st = { "working", "blue", ring = true }
      elseif status == "running" then st = { "running", "green" }
      elseif status == "stopped" then st = { "stopped", "text-muted", ring = true }
      else st = { status, "red" } end
    else
      local s0 = j.status
      if s0 == "running" and j.working then st = { "working", "blue", ring = true }
      elseif s0 == "running" then st = { "running", "green" }
      elseif s0 == "starting" or s0 == "creating" or s0 == "unknown" then st = { s0, "blue", ring = true }
      elseif s0 == "failed" then st = { "failed", "red" }
      else st = { s0 == "absent" and "not created" or s0, "text-muted", ring = true } end
    end
    -- The dot as the sidebar draws it: filled, or a ring for stopped and for a state in progress.
    local state = row{ gap = 8, min_w = 0, clip = true, key = target and "jarvis-target-state" or "jarvis-state",
      help = target and (target .. "'s agent: " .. st[1]) or ("Jarvis: " .. st[1]),
      column{ w = 8, h = 8, radius = "pill", fill = (not st.ring) and st[2] or nil, border = st.ring and st[2] or nil },
      -- With a proposal waiting its button needs the room: the dot alone (its tooltip says the word).
      #j.pending == 0 and text{ st[1], size = "sm", color = st[2], truncate = true } or false,
    }

    -- The shared panel header: the mark and the title at the left, where the
    -- words go (a compact picker, sized to its label, only when there is
    -- more than Jarvis to pick), the state; what waits and the cross at the
    -- right. The state's word gives way first (it truncates; its dot stays).
    local head = row{ w = "full", gap = 8, h = 28,
      row{ gap = 8, grow = 1, min_w = 0, clip = true,
        icon{ "jarvis", color = "purple-alt" },
        text{ "Jarvis", size = "title", weight = "semibold", color = "text-bright" },
        #items > 1 and row{ gap = 4,
          icon{ "send", size = 12, color = "text-muted", help = "Where your words go" },
          -- A sandbox picked: its colour square, as the overlays' sandbox picker.
          target and column{ w = 8, h = 8, radius = "mark", key = "jarvis-target-color",
            fill = (((data.app or {}).groups or {})[target]) or "text-dim" } or false,
          select{ id = "jarvis-target", items = items, value = target or "sbxw-jarvis", on = "target", min_w = 0,
            help = "Where your words go: Jarvis, or a running sandbox's agent (you confirm each message)" },
        } or false,
        state,
      },
      #j.pending > 0 and button{ #j.pending == 1 and "1 to review" or (#j.pending .. " to review"),
        on = "review", arg = j.pending[1].id, tone = "amber", key = "jarvis-review",
        help = "Jarvis proposes: " .. j.pending[1].summary } or false,
      button{ icon = "close", variant = "ghost", on = "close", key = "jarvis-close", help = "Close the Jarvis panel" },
    }

    -- The picked sandbox: Follow, on a line of its own.
    local sub = target and row{ w = "full", gap = 8,
      checkbox{ "Follow", value = f.follow == true, on = "follow", key = "jarvis-follow" },
      text{ f.follow and "every turn shown and read" or "rounds only", size = "sm", color = "text-muted", truncate = true, grow = 1 },
    } or false

    -- Jarvis's own state and Start button are its business, not the picked sandbox's.
    local note = (not target) and send_note(j, state) or false
    local log = {}
    if not running and #j.log == 0 and not target then
      local why = WHY[j.status] or ""
      if j.status == "failed" then why = "Jarvis could not start: " .. (j.error or "unknown error") end
      log[#log + 1] = text{ why, wrap = true, color = "text-muted" }
      if (j.status == "absent" or j.status == "stopped" or j.status == "failed") and not note then
        log[#log + 1] = row{ button{ "Start Jarvis", icon = "play", variant = "primary", on = "start" } }
      elseif j.status == "creating" or j.status == "starting" then
        log[#log + 1] = progress{ value = 0.5, tone = "blue" }
      end
    end
    for _, m in ipairs(j.log) do
      log[#log + 1] = message(m, v.speaks)
    end
    if not target and (j.working or j.sending) then
      log[#log + 1] = text{ "…", color = "text-muted", key = "jarvis-working" }
    end
    if not target and not running and #j.log > 0 and not note and (j.status == "absent" or j.status == "stopped" or j.status == "failed") then
      log[#log + 1] = row{ button{ "Start Jarvis", icon = "play", variant = "primary", on = "start" } }
    end
    log.gap = 12
    log.stick_bottom = true
    log.key = "jarvis-log"

    -- The composer: the app's input, then its icon buttons (14 pt in a
    -- 24 pt target, ghost, their keys in the tooltip). While Jarvis is
    -- stopped the placeholder says so; typing still works (the send offers
    -- to start it), so nothing is disabled.
    local talk = false
    if v.hears then
      talk = button{ icon = "mic", on = "talk", arg = v.recording, key = "jarvis-talk",
        variant = v.recording and "danger" or "ghost",
        keys = (not v.recording and not v.key_note) and { v.key } or nil,
        help = v.recording and "Recording: click again to send"
          or (v.key_note or "Hold to talk, or click to start and again to send") }
    end
    return column{ w = "full", h = "full", gap = 8,
      head,
      sub,
      -- An inline notice, as the app's: quiet, an info mark, its own line.
      (j.others or 0) > 0 and row{ w = "full", gap = 8, key = "jarvis-others",
        icon{ "info", size = 12, color = "text-muted" },
        text{ "sbxw is also open in another window or browser tab: Jarvis may be read aloud there too.",
          wrap = true, size = "sm", color = "text-muted", grow = 1 },
      } or false,
      scroll(log),
      v.recording and progress{ value = v.level, tone = "red" } or false,
      note,
      row{ w = "full", gap = 8,
        column{ grow = 1, input{ id = "jarvis-input", on_submit = "send", on = "edited", tone = "compose",
          placeholder = target and ("Message " .. target .. " (you confirm it first)") or (running and "Ask Jarvis" or "Ask Jarvis (start it first)") } },
        talk,
        button{ icon = "send", on = "send", key = "jarvis-send", variant = "ghost", keys = { "Enter" }, help = "Send" },
      },
    }
  end,

  actions = {
    send = function(state) state.edited = nil; return intent("send_to_jarvis", { input = "jarvis-input" }) end,
    edited = function(state, _, text) state.edited = text or "" end,
    start = function() return intent("start_jarvis") end,
    -- One press, both intents: the core sends the input once Jarvis runs.
    start_send = function(state)
      state.edited = nil
      return { intent("start_jarvis"), intent("send_to_jarvis", { input = "jarvis-input" }) }
    end,
    close = function() return intent("toggle_jarvis_panel", { open = false }) end,
    review = function(_, id) return intent("approve_proposal", { id = id }) end,
    talk = function(_, recording)
      if recording then return intent("talk_stop") end
      return intent("talk_start")
    end,
    read = function(_, id) return intent("read_aloud", { id = id }) end,
    link = function(_, _, url) return intent("open_link", { url = url }) end,
    target = function(_, _, id)
      if id == "sbxw-jarvis" then return intent("follow_target", {}) end
      return intent("follow_target", { sandbox = id })
    end,
    follow = function(_, _, on) return intent("follow_toggle", { on = on }) end,
  },
}
