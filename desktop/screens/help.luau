-- Help: why the sandbox list may be empty (the web UI's help dialog), the
-- keyboard shortcuts, and the versions. A dialog over the window.

local function step(n, title, body, cmd)
  return column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
    row{ gap = 8, badge{ tostring(n), tone = "blue" }, text{ title, weight = "semibold", color = "text-bright" } },
    text{ body, size = "sm", color = "text-muted", wrap = true },
    cmd and text{ cmd, mono = true, size = "sm", color = "text-bright", wrap = true } or false,
  }
end

-- One shortcut: its keycaps, then what it does (data.app.shortcuts, the
-- core's own list, so Help never disagrees with the keys).
local function key(keys, what)
  local caps = { gap = 4, w = 168 }
  for _, k in ipairs(keys) do table.insert(caps, badge{ k, tone = "neutral" }) end
  return row{ gap = 12, w = "full",
    row(caps),
    text{ what, size = "body", color = "text", wrap = true, grow = 1 },
  }
end

local function shortcut_rows(list)
  local rows = { gap = 8 }
  for _, s in ipairs(list) do table.insert(rows, key(s.keys, s.what)) end
  table.insert(rows, key({ "Click a pane" }, "Give it the keyboard"))
  return rows
end

return {
  render = function(_, data)
    local mac = data.app.platform == "macos"
    local voice = (data.jarvis or {}).voice or {}
    local ptt = voice.push_to_talk_key or "F8"
    local v = ((data.daemon_state or {}).versions) or {}
    return column{ h = "full", w = "full", gap = 12,
      row{ w = "full", text{ "Help", size = "lg", weight = "semibold", color = "text-bright" }, spacer{},
        button{ icon = "close", variant = "ghost", on = "close", key = "help-close", help = "Close" } },
      scroll{ gap = 12,
        text{ "No sandboxes showing?", weight = "semibold", color = "text-bright" },
        text{ "The list shows exactly what the sbx CLI reports. An empty list nearly always means sbx has nothing to report, not that your sandboxes are gone. Run these on the machine sbxw runs on (a terminal on your host, not a sandbox pane).", size = "sm", color = "text-muted", wrap = true },
        step(1, "Log in to sbx", "An unauthenticated CLI lists no sandboxes at all. Once per machine; it opens a browser.", "sbx login"),
        step(2, "Restart the sbx daemon", "Run sbx first: after an update it may suggest a restart. Still empty? Start its daemon again in the background; it writes to ~/.sbx/daemon-restart.log.", "nohup sbx daemon start >> ~/.sbx/daemon-restart.log 2>&1 &"),
        divider{},
        text{ "Where things are", weight = "semibold", color = "text-bright" },
        text{ "The top bar: GUI | TUI (Sessions or the terminals), Lead Mode (only the team leaders) and Kanban (the tickets planned, in progress, in test, ready to merge and merged; pressed again it closes).", size = "sm", color = "text-muted", wrap = true },
        text{ "The rail on the far left: at the top, what happens now (Requests, Network, the Dashboard: the teams' conversations, with the same Kanban one switch away); at the bottom, what is set up. MCP servers (tools a sandbox can load), Skills (kept for every new sandbox) and Kits (what a project's sandboxes start with; amber with a number when one waits for your answer) each open a panel on the right. Settings holds secrets and sign-in, remote hosts and the voice models. A sandbox's Manage dialog and a pane's ⋯ menu open the same panels on that sandbox.", size = "sm", color = "text-muted", wrap = true },
        divider{},
        text{ "Keyboard shortcuts", weight = "semibold", color = "text-bright" },
        list(shortcut_rows(data.app.shortcuts or {})),
        list{ gap = 8, key({ mac and "⌘" or "Ctrl", "Click" }, "On a file path in a terminal: open it in Files (a link opens in the browser). Add Shift when the program uses the mouse.") },
        divider{},
        text{ "sbxw " .. (v.sbxw or "?") .. " · sbx " .. (v.sbx or "?") .. " · " .. (data.app.connection.text or ""), size = "sm", color = "text-muted", wrap = true },
      },
    }
  end,
  actions = {
    close = function() return intent("close_modal") end,
  },
}
