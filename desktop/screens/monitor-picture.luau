-- A picture the person sent, opened large (feat/monitor/pics.rs): the
-- picture fitted to the window with a margin, never larger than its own
-- pixels; under it its file's name (cut, made safe by the Rust) and its
-- size in pixels, and Close. Escape closes it too (the core's). While the
-- large copy loads, the small preview and one line; a failure, its muted
-- line. Nothing else: no download, no copy, no other app.

-- The dialog's card (shell/features.rs `modal_view`): 1080 wide at most,
-- 16 of side margin, its top 76 down and 24 above the window's foot, 720
-- tall at most, 16 of padding. Below the picture: a gap and the foot.
local CARD_W, PAD, FOOT = 1080, 16, 12 + 32 + 8

local function bounds(win)
  local w = math.min(CARD_W, (win.w or 800) - 32) - 2 * PAD
  local card_h = math.max(160, math.min(720, (win.h or 600) - 76 - 24))
  local h = card_h - 2 * PAD - FOOT
  return math.max(40, w), math.max(40, h)
end

local function note(t)
  return text{ t, size = "sm", color = "text-muted", wrap = true }
end

return {
  render = function(_, data)
    local p = data.monitor_picture or {}
    local mw, mh = bounds((data.app or {}).window or {})
    local body
    if p.state == "ready" and p.key then
      body = image{ key = p.key, max_w = mw, max_h = mh }
    elseif p.state == "note" then
      body = note(p.note or "Picture cannot be previewed")
    else
      local small = p.small or {}
      body = column{ gap = 8, align = "center",
        small.key and image{ key = small.key, max_w = mw, max_h = math.max(40, mh - 28) } or false,
        note("Loading the full picture…") }
    end
    local size = (p.w and p.h) and (p.w .. " × " .. p.h .. " px") or ""
    return column{ gap = 12, w = "full", key = "monitor-picture",
      row{ w = "full", justify = "center", key = "monitor-picture-frame", body },
      row{ gap = 8, w = "full", key = "monitor-picture-foot",
        text{ p.name or "", truncate = true, grow = 1, color = "text-bright", key = "monitor-picture-name", help = p.name },
        size ~= "" and text{ size, size = "sm", color = "text-muted", key = "monitor-picture-size" } or false,
        button{ "Close", on = "close", key = "monitor-picture-close", help = "Close the picture (Escape)" } } }
  end,
  actions = {
    close = function() return intent("monitor_picture_close", {}) end,
  },
}
