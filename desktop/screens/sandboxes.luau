-- The sandbox list. Edit this file while the app runs: the panel redraws
-- and keeps `state` (the selection, the click count).

local colors = { running = "#a6e3a1", stopped = "#6c7086" }

return {
  init = function()
    return { selected = nil, clicks = 0 }
  end,

  render = function(state, data)
    local rows = {}
    for _, sb in ipairs(data.sandboxes) do
      local mark = (state.selected == sb.name) and "> " or "  "
      table.insert(rows, row{
        text{ mark .. sb.name, bold = state.selected == sb.name },
        text{ sb.status, color = colors[sb.status] or "#f9e2af" },
        button{ "select", on = "select", arg = sb.name },
      })
    end
    return column{
      text{ "Sandboxes", bold = true },
      text{ "from " .. data.source, color = "#6c7086" },
      list(rows),
      text{ "selected: " .. (state.selected or "none") .. ", clicks: " .. state.clicks },
    }
  end,

  actions = {
    select = function(state, name)
      state.selected = name
      state.clicks = state.clicks + 1
    end,
  },
}
