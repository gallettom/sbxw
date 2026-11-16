-- Help: why the sandbox list may be empty (the web UI's help dialog), the
-- keyboard shortcuts, and the versions. A dialog over the window.

local function step(n, title, body, cmd)
  return column{ gap = 4, w = "full", pad = 8, radius = "sm", fill = "surface-alt",
    row{ gap = 8, badge{ tostring(n), tone = "blue" }, text{ title, weight = "semibold", color = "text-bright" } },
    text{ body, size = "sm", color = "text-muted", wrap = true },
    cmd and text{ cmd, mono = true, size = "sm", color = "text-bright", wrap = true } or false,
  }
end

local function key(k, what)
  return row{ gap = 10, w = "full",
    row{ w = 150, badge{ k, tone = "neutral" } },
    text{ what, size = "sm", color = "text", wrap = true, grow = 1 },
  }
end

return {
  render = function(_, data)
    local mac = data.app.platform == "macos"
    local voice = (data.jarvis or {}).voice or {}
    local ptt = voice.push_to_talk_key or "F8"
    local v = ((data.daemon_state or {}).versions) or {}
    return column{ h = "full", w = "full", gap = 10,
      row{ w = "full", text{ "Help", size = "lg", weight = "semibold", color = "text-bright" }, spacer{},
        button{ icon = "close", variant = "ghost", on = "close", key = "help-close", help = "Close" } },
      scroll{ gap = 12,
        text{ "No sandboxes showing?", weight = "semibold", color = "text-bright" },
        text{ "The list shows exactly what the sbx CLI reports. An empty list nearly always means sbx has nothing to report, not that your sandboxes are gone. Run these on the machine sbxw runs on (a terminal on your host, not a sandbox pane).", size = "sm", color = "text-muted", wrap = true },
        step(1, "Log in to sbx", "An unauthenticated CLI lists no sandboxes at all. Once per machine; it opens a browser.", "sbx login"),
        step(2, "Restart the sbx daemon", "Run sbx first: after an update it may suggest a restart. Still empty? Start its daemon again in the background; it writes to ~/.sbx/daemon-restart.log.", "nohup sbx daemon start >> ~/.sbx/daemon-restart.log 2>&1 &"),
        divider{},
        text{ "Keyboard shortcuts", weight = "semibold", color = "text-bright" },
        list{
          key(mac and "⌘J" or "F7", "Open or close the Jarvis panel"),
          key(ptt, "Push to talk to Jarvis (hold)"),
          key("Escape", "Close the dialog or menu showing"),
          key(mac and "⌘V" or "Ctrl+Shift+V", "Paste into the focused terminal"),
          key("Shift+PageUp / PageDown", "Scroll a terminal's history"),
          key("Click a pane", "Give it the keyboard"),
        },
        divider{},
        text{ "sbxw " .. (v.sbxw or "?") .. " · sbx " .. (v.sbx or "?") .. " · " .. (data.app.connection.text or ""), size = "sm", color = "text-dim", wrap = true },
      },
    }
  end,
  actions = {
    close = function() return intent("close_modal") end,
  },
}
