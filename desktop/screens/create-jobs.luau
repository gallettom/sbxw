-- Sandboxes being created (the web UI's corner cards, assets/js/sandboxes.js):
-- every step of the plan the daemon announced, the running one with what sbx
-- prints under it, the failure with its reason, Retry and Dismiss. Docked on
-- the right while a bring-up runs; the core opens the new sandbox's terminal
-- when it is ready.

local MARK = { done = "check", running = "reload", failed = "close", waiting = "chevron-right" }
local TONE = { done = "green", running = "blue", failed = "red", waiting = "text-dim" }

local function job(j)
  local kids = { w = "full", gap = 4, pad = 12, radius = "md", border = j.state == "failed" and "red-border" or "border",
    fill = "surface", key = "job-" .. j.sandbox,
    row{ w = "full", gap = 8,
      dot{ tone = j.state == "ready" and "green" or j.state == "failed" and "red" or j.state == "asking" and "amber" or "blue" },
      text{ j.sandbox, weight = "semibold", color = "text-bright", truncate = true, grow = 1 },
      text{ j.headline, size = "sm", color = j.state == "failed" and "red" or j.state == "asking" and "amber" or "text-muted" },
    },
  }
  local total = #j.steps
  if j.state == "running" and total > 0 then
    local done = 0
    for _, s in ipairs(j.steps) do if s.state == "done" then done = done + 1 end end
    kids[#kids + 1] = progress{ value = done / total, tone = "blue" }
  elseif j.state == "running" then
    kids[#kids + 1] = text{ "Waiting for the daemon to say what it will do...", size = "sm", color = "text-muted", wrap = true }
  end
  for i, s in ipairs(j.steps) do
    kids[#kids + 1] = row{ w = "full", gap = 8, key = "job-" .. j.sandbox .. "-step-" .. i,
      icon{ MARK[s.state] or "chevron-right", size = 11, color = TONE[s.state] or "text-dim" },
      text{ s.label, size = "sm", color = s.state == "waiting" and "text-dim" or "text", truncate = true, grow = 1 },
    }
    if s.detail then
      kids[#kids + 1] = text{ s.detail, mono = true, size = "xs", color = "text-muted", truncate = true, help = s.detail }
    end
  end
  if j.error then
    kids[#kids + 1] = text{ j.error, wrap = true, size = "sm", color = j.state == "asking" and "amber" or "red", key = "job-" .. j.sandbox .. "-error" }
  end
  for _, w in ipairs(j.warnings or {}) do
    kids[#kids + 1] = row{ w = "full", gap = 8, icon{ "warning", size = 12, color = "amber" },
      text{ w, wrap = true, size = "sm", color = "amber", grow = 1 } }
  end
  local buttons = { w = "full", gap = 8, spacer{} }
  if j.state == "ready" then
    buttons[#buttons + 1] = button{ "Open", icon = "terminal", variant = "primary", on = "open", arg = j.sandbox, key = "job-open-" .. j.sandbox }
  end
  if j.can_retry then
    buttons[#buttons + 1] = button{ "Retry", icon = "reload", variant = "primary", on = "retry", arg = j.sandbox, key = "job-retry-" .. j.sandbox }
  end
  if j.state ~= "running" then
    buttons[#buttons + 1] = button{ "Dismiss", variant = "ghost", on = "dismiss", arg = j.sandbox, key = "job-dismiss-" .. j.sandbox }
  end
  if #buttons > 2 then kids[#kids + 1] = row(buttons) end
  return column(kids)
end

return {
  render = function(_, data)
    local jobs = (data.create and data.create.jobs) or {}
    local list = { gap = 12 }
    if #jobs == 0 then
      list[#list + 1] = text{ "Nothing is being created.", size = "sm", color = "text-muted" }
    end
    for _, j in ipairs(jobs) do list[#list + 1] = job(j) end
    return column{ w = "full", h = "full", gap = 8,
      row{ w = "full", gap = 8,
        icon{ "plus", color = "blue" },
        text{ "New sandboxes", weight = "semibold", color = "text-bright" },
        spacer{},
        button{ "New", icon = "plus", variant = "ghost", on = "new", key = "jobs-new", help = "Create another sandbox" },
      },
      scroll(list),
    }
  end,

  actions = {
    open = function(_, name)
      return { intent("select", { sandbox = name }), intent("open_terminal", { sandbox = name, pane = "spare" }) }
    end,
    retry = function(_, name) return intent("create_retry", { sandbox = name }) end,
    dismiss = function(_, name) return intent("create_dismiss", { sandbox = name }) end,
    new = function() return intent("open_modal", { screen = "create", arg = "chooser" }) end,
  },
}
