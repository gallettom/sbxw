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
      column{ max_w = 300, px = 10, py = 6, radius = "sm", fill = "elevated",
        text{ m.text, wrap = true, color = "text-bright" },
      },
    }
  elseif m.who == "jarvis" then
    return column{ w = "full", gap = 2,
      markdown{ m.text, on = "link", key = "msg-" .. m.id },
      speaks and row{ w = "full", justify = "end",
        button{ icon = "speaker", variant = "ghost", on = "read", arg = m.id,
          color = m.reading and "blue" or "text-dim",
          help = m.reading and "Stop reading" or "Read aloud" },
      } or false,
    }
  elseif m.who == "error" then
    return text{ m.text, wrap = true, size = "sm", color = "red" }
  end
  return text{ m.text, wrap = true, size = "sm", color = "text-muted" }
end

return {
  render = function(_, data)
    local j = data.jarvis
    local v = j.voice
    local running = j.status == "running"

    local head = row{ w = "full", gap = 8, h = 28,
      icon{ "sparkle", color = "blue" },
      text{ "Jarvis", weight = "semibold", color = "text-bright" },
      (not running) and text{ j.status, size = "sm", color = "text-muted" } or false,
      spacer{},
      #j.pending > 0 and button{ #j.pending == 1 and "1 to review" or (#j.pending .. " to review"),
        on = "review", arg = j.pending[1].id, variant = "primary",
        help = "Jarvis proposes: " .. j.pending[1].summary } or false,
      button{ icon = "close", variant = "ghost", on = "close", help = "Close the Jarvis panel" },
    }

    local log = {}
    if not running and #j.log == 0 then
      local why = WHY[j.status] or ""
      if j.status == "failed" then why = "Jarvis could not start: " .. (j.error or "unknown error") end
      log[#log + 1] = text{ why, wrap = true, color = "text-muted" }
      if j.status == "absent" or j.status == "stopped" or j.status == "failed" then
        log[#log + 1] = row{ button{ "Start Jarvis", icon = "play", variant = "primary", on = "start" } }
      elseif j.status == "creating" or j.status == "starting" then
        log[#log + 1] = progress{ value = 0.5, tone = "blue" }
      end
    end
    for _, m in ipairs(j.log) do
      log[#log + 1] = message(m, v.speaks)
    end
    if j.working or j.sending then
      log[#log + 1] = text{ "…", color = "text-muted", key = "jarvis-working" }
    end
    if not running and #j.log > 0 and (j.status == "absent" or j.status == "stopped" or j.status == "failed") then
      log[#log + 1] = row{ button{ "Start Jarvis", icon = "play", variant = "primary", on = "start" } }
    end
    log.gap = 12
    log.stick_bottom = true
    log.key = "jarvis-log"

    local talk = false
    if v.hears then
      talk = button{ icon = "mic", on = "talk", arg = v.recording,
        variant = v.recording and "danger" or "ghost",
        help = v.recording and "Recording: click again to send"
          or (v.key_note or ("Hold " .. v.key .. " to talk, or click to start and again to send")) }
    end

    return column{ w = "full", h = "full", gap = 8,
      head,
      scroll(log),
      v.recording and progress{ value = v.level, tone = "red" } or false,
      row{ w = "full", gap = 6,
        column{ grow = 1, input{ id = "jarvis-input", on_submit = "send",
          placeholder = running and "Ask Jarvis" or "Ask Jarvis (start it first)" } },
        talk,
        button{ icon = "send", on = "send", help = "Send (Enter)" },
      },
    }
  end,

  actions = {
    send = function() return intent("send_to_jarvis", { input = "jarvis-input" }) end,
    start = function() return intent("start_jarvis") end,
    close = function() return intent("toggle_jarvis_panel", { open = false }) end,
    review = function(_, id) return intent("approve_proposal", { id = id }) end,
    talk = function(_, recording)
      if recording then return intent("talk_stop") end
      return intent("talk_start")
    end,
    read = function(_, id) return intent("read_aloud", { id = id }) end,
    link = function(_, _, url) return intent("open_link", { url = url }) end,
  },
}
