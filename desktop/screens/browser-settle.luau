-- The question asked when a sandbox's browser closed (the web UI's
-- browser-settle dialog): the hosts allowed from the Browser panel, for that
-- sandbox only, are kept or removed. `data.browser_settle` = { sandbox, hosts }.
-- "Keep them" comes first: the answer that removes nothing.

return {
  render = function(_, data)
    local q = data.browser_settle or {}
    local hosts = {}
    for _, h in ipairs(q.hosts or {}) do
      table.insert(hosts, text{ h, mono = true, size = "sm", color = "text-bright" })
    end
    return column{ gap = 12, pad = 16, w = "full",
      text{ "The browser of " .. (q.sandbox or "?") .. " closed", weight = "semibold", size = "lg", color = "text-bright" },
      text{ "These hosts were allowed from the browser, for this sandbox only. Keep them, or remove their network rules?", size = "sm", color = "text-muted", wrap = true },
      #hosts > 0 and list(hosts) or false,
      row{ gap = 8, w = "full", justify = "end",
        button{ "Remove them", on = "settle", arg = false, variant = "danger", key = "browser-settle-remove" },
        button{ "Keep them", on = "settle", arg = true, variant = "primary", key = "browser-settle-keep" },
      },
      text{ "Escape leaves the question for the next close: the hosts stay allowed meanwhile.", size = "xs", color = "text-dim", wrap = true },
    }
  end,
  actions = {
    settle = function(_, keep) return intent("browser_settle", { keep = keep }) end,
  },
}
