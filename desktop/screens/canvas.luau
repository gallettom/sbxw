-- Canvas: the shots an agent added of the screens it works on, to review.
-- The surface (`canvas{ id = "canvas" }`) is drawn by the core: pan, zoom,
-- pins, notes and pictures never run this file. Everything here comes from
-- data.canvas_ui, built by the feature (src/feat/canvas/). Texts from an
-- agent are drawn as text only.

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

-- The pickers beside the overlay's title, in ONE function so a shared
-- scope picker can replace them: the sandbox, then the canvas.
local function pickers(data, ui)
  local cur = ui.current or {}
  local w = (((data.app or {}).overlay or {}).w or 1000)
  return row{ key = "title-slot", gap = 8,
    sandbox_picker(data, { id = "canvas-sandbox", value = ui.sandbox, items = ui.sandboxes or {}, on = "sandbox", placeholder = "pick a sandbox" }),
    menu{ cur.title and (w >= 600 and cur.title or "Canvas") or "No canvas", id = "canvas-pick", items = ui.canvases or {}, on = "pick", icon = "canvas" },
    w >= 900 and text{ ui.storage or "", size = "xs", color = "text-muted", key = "canvas-storage" } or false,
  }
end

local TOOLS = {
  { "select", "pointer", "Select", "Select, move and resize; drag the ground to pan", "V" },
  { "pin", "pin", "Pin", "Pin: press a point of a shot", "P" },
  { "note", "note", "Note", "Note: press where it goes", "N" },
  { "asset", "image", "Picture", "Example picture: press where it goes, then pick the file", "A" },
}

local function toolbar(ui, wide)
  local out = { gap = 4, w = "full", key = "canvas-tools" }
  for _, t in ipairs(TOOLS) do
    local on_it = ui.tool == t[1]
    out[#out + 1] = button{ wide and t[3] or nil, icon = t[2], key = "canvas-tool-" .. t[1], on = "tool", arg = t[1], dense = true,
      variant = on_it and "neutral" or "ghost", color = on_it and "text-bright" or "text-muted", help = t[4], keys = { t[5] } }
  end
  out[#out + 1] = spacer{}
  out[#out + 1] = text{ ui.zoom or "", size = "xs", color = "text-muted", key = "canvas-zoom-readout", help = "The zoom: 100% shows one CSS pixel of a shot per point" }
  out[#out + 1] = button{ icon = "minus", key = "canvas-zoom-out", on = "zoom", arg = "out", variant = "ghost", dense = true, help = "Zoom out (pinch, or ⌘ and scroll)" }
  out[#out + 1] = button{ icon = "plus", key = "canvas-zoom-in", on = "zoom", arg = "in", variant = "ghost", dense = true, help = "Zoom in" }
  out[#out + 1] = button{ icon = "fit", key = "canvas-zoom-fit", on = "zoom", arg = "fit", variant = "ghost", dense = true, help = "Every shot in view" }
  out[#out + 1] = button{ "100%", key = "canvas-zoom-actual", on = "zoom", arg = "actual", variant = "ghost", dense = true, help = "One CSS pixel of the shot per point" }
  return row(out)
end

local function ask_block(ui, data)
  local s = ui.send or {}
  return column{ gap = 8, w = "full", key = "canvas-ask",
    note("Shots come from the agent: it takes screenshots of the app it works on and adds them with its canvas_add_shots tool. You can ask it here; the prompt goes only after you press Ask and confirm."),
    textarea{ id = "canvas-ask-text", value = (ui.ask or {}).text or "", rows = 4, on = "ask_text" },
    row{ gap = 8, w = "full",
      button{ "Ask the agent for screenshots", icon = "send", key = "canvas-ask-go", on = "ask", variant = "primary", disabled = not s.ask_ok,
        help = s.ask_ok and ("Types this prompt into " .. (s.to or "the session") .. " after one confirmation") or (s.why or "") },
      text{ s.to or "", size = "xs", color = "text-muted", truncate = true, grow = 1 } } }
end

local function remark(d)
  local head = row{ gap = 8, w = "full", key = "canvas-row-" .. d.key, on = "select", arg = d.key, hover = "hover-subtle", radius = "xs", px = 4, py = 2,
    fill = d.selected and "active-bg" or nil, help = "Show it on the canvas",
    icon{ d.kind == "pin" and "pin" or (d.kind == "note" and "note" or "image"), size = 12, color = d.kind == "pin" and "green" or (d.kind == "note" and "amber" or "blue") },
    text{ d.label, size = "sm", color = "text-bright", truncate = true, grow = 1 },
    d.look and badge{ "look", tone = "amber", help = "Its name or folder looks like a secret: the send asks you to look first" } or false,
    button{ icon = "trash", key = "canvas-remove-" .. d.key, on = "remove", arg = d.key, variant = "ghost", dense = true, tab_stop = false, help = "Remove it (it was not sent)" } }
  if d.kind == "asset" then return head end
  return column{ gap = 4, w = "full", key = "canvas-remark-" .. d.key, head,
    textarea{ id = "canvas-text-" .. d.key, value = d.text or "", rows = d.selected and 4 or 2, on = "text", arg = d.key, tone = "compose",
      placeholder = d.kind == "pin" and "What should change here?" or "A remark about the screens" } }
end

local function side(ui, data)
  local out = { gap = 12, w = "full" }
  local draft = ui.draft or {}
  out[#out + 1] = row{ gap = 8, w = "full", text{ "New remarks", weight = "semibold", color = "text-bright", grow = 1 },
    badge{ tostring(#draft), tone = #draft > 0 and "green" or "neutral" } }
  if #draft == 0 then
    out[#out + 1] = note("Pick Pin (P) and press a point of a shot, Note (N) or a picture (A). Your remarks are saved as you go and sent together.")
  end
  for _, d in ipairs(draft) do out[#out + 1] = remark(d) end
  local hist = ui.history or {}
  if #hist > 0 then
    out[#out + 1] = divider{}
    out[#out + 1] = text{ "Sent rounds", weight = "semibold", color = "text-bright" }
    for _, h in ipairs(hist) do
      local items = { gap = 4, w = "full", key = "canvas-review-" .. h.n, text{ h.line, size = "xs", color = "text-muted", wrap = true } }
      for _, it in ipairs(h.items or {}) do
        items[#items + 1] = column{ gap = 2, w = "full", px = 8, py = 4, radius = "xs", fill = "elevated", key = "canvas-row-sent-" .. it.key, on = "select", arg = it.key, help = "Show it on the canvas",
          text{ it.label, size = "xs", color = "text-muted", truncate = true },
          it.text ~= "" and text{ it.text, size = "sm", color = "text", wrap = true } or false }
      end
      out[#out + 1] = column(items)
    end
  end
  return column(out)
end

local function sendbar(ui)
  local s = ui.send or {}
  local sessions = s.sessions or {}
  return column{ gap = 8, w = "full", key = "canvas-sendbar",
    #sessions > 1 and select{ id = "canvas-session", value = s.session or "", items = sessions, on = "session" } or false,
    button{ s.label or "Send remarks", icon = "send", key = "canvas-send", on = "send", variant = "primary", disabled = not s.ok,
      help = s.ok and ("Sends " .. tostring(s.new or 0) .. " new remark(s) after one confirmation") or (s.why or "") },
    (not s.ok and s.why) and text{ s.why, size = "xs", color = "text-muted", wrap = true } or false }
end

return {
  render = function(_, data)
    local ui = data.canvas_ui or {}
    local wide = overlay_wide(data)
    local top = pickers(data, ui)
    local head = { gap = 8, w = "full", top }
    if ui.error then head[#head + 1] = note(ui.error, "red") end
    if ui.removed then head[#head + 1] = note("This sandbox was removed: its canvases stay here until you delete them.", "amber") end
    if ui.renaming then
      head[#head + 1] = row{ gap = 8, w = "full", key = "canvas-rename",
        input{ id = "canvas-title", value = (ui.current or {}).title or "", on_submit = "rename" },
        button{ "Cancel", key = "canvas-rename-cancel", on = "rename_close", variant = "ghost" } }
    end
    if not ui.current then
      head[#head + 1] = scroll{ key = "canvas-empty-scroll", column{ gap = 12, w = "full", py = 24, key = "canvas-empty",
        text{ ui.empty == "loading" and "Reading the canvas…" or "No canvas for this sandbox yet", weight = "semibold", color = "text-bright" },
        note("A canvas collects screenshots of the app an agent works on. Zoom and move over them, pin remarks on a point, add notes and example pictures, then send your remarks: the agent reads them with its canvas_read tool."),
        row{ gap = 8, button{ "New canvas", icon = "plus", key = "canvas-new", on = "pick", arg = "new", disabled = (ui.sandbox or "") == "" } },
        ask_block(ui, data) } }
      head.h = "full"
      return column(head)
    end
    local actions = row{ gap = 4,
      button{ "Rename", key = "canvas-rename-open", on = "rename_open", variant = "ghost", dense = true },
      menu{ "Delete", id = "canvas-delete", variant = "ghost", on = "delete", icon = "trash", items = {
        { id = "this", label = "Delete this canvas…", icon = "trash", tone = "red" },
        { divider = true },
        { id = "7", label = "Delete canvases older than 7 days…" },
        { id = "30", label = "Delete canvases older than 30 days…" },
        { id = "90", label = "Delete canvases older than 90 days…" } } },
      (not wide) and text{ ui.storage or "", size = "xs", color = "text-muted", truncate = true } or false }
    local surface = column{ grow = 1, h = "full", min_w = 0, min_h = 0, gap = 8, key = "canvas-surface-col",
      toolbar(ui, wide),
      ui.empty == "shots" and ask_block(ui, data) or false,
      canvas{ id = "canvas" } }
    local panel = column{ w = wide and 320 or "full", h = "full", gap = 8,
      column{ grow = 1, min_h = 0, w = "full", scroll{ key = "canvas-side", side(ui, data) } }, sendbar(ui) }
    head[#head + 1] = actions
    if wide then
      head[#head + 1] = row{ grow = 1, min_h = 0, w = "full", gap = 12, align = "stretch", surface, panel }
      head.h = "full"
      return column(head)
    end
    local tabs = row{ gap = 4, w = "full",
      button{ "Canvas", key = "canvas-tab-canvas", on = "tab", arg = "canvas", dense = true, variant = ui.tab == "canvas" and "neutral" or "ghost" },
      button{ "Remarks (" .. tostring(#(ui.draft or {})) .. ")", key = "canvas-tab-remarks", on = "tab", arg = "remarks", dense = true, variant = ui.tab == "remarks" and "neutral" or "ghost" } }
    head[#head + 1] = tabs
    head[#head + 1] = column{ grow = 1, min_h = 0, w = "full", ui.tab == "remarks" and panel or surface }
    head.h = "full"
    return column(head)
  end,
  actions = {
    sandbox = function(_, _, v) return intent("canvas_sandbox", { name = v }) end,
    pick = function(_, a, v) return intent("canvas_pick", { id = v or a }) end,
    rename_open = function() return intent("canvas_rename", { open = true }) end,
    rename_close = function() return intent("canvas_rename", { open = false }) end,
    rename = function(_, _, v) return intent("canvas_rename", { title = v or "" }) end,
    delete = function(_, _, v)
      if v == "this" then return intent("canvas_delete", {}) end
      return intent("canvas_delete", { older = v })
    end,
    tool = function(_, t) return intent("canvas_tool", { tool = t }) end,
    zoom = function(_, c) return intent("canvas_zoom", { cmd = c }) end,
    select = function(_, k) return intent("canvas_select", { target = k }) end,
    text = function(_, k, v) return intent("canvas_text", { target = k, text = v or "" }) end,
    remove = function(_, k) return intent("canvas_remove", { target = k }) end,
    session = function(_, _, v) return intent("canvas_session", { key = v }) end,
    send = function() return intent("canvas_send", {}) end,
    ask = function() return intent("canvas_ask", {}) end,
    ask_text = function(_, _, v) return intent("canvas_ask_text", { text = v or "" }) end,
    tab = function(_, t) return intent("canvas_tab", { tab = t }) end,
  },
}
