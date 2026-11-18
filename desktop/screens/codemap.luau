-- The Code map panel (docked on the right): the linked Markdown a /codemap
-- run leaves under .sbxw-artifacts/codemap/, and its lenses
-- (assets/js/codemap.js). A [[wiki link]] is followed inside the panel; each
-- file lists what links to it, its outline and the source it names.
--
-- The map was written by an agent: it is drawn through `markdown`, never
-- run. Its Archify diagram is an HTML page the app does not draw: Diagram
-- asks the core to save it and open it in the browser, after its dialog.

local function nz(s) return s ~= nil and s ~= "" end

local function note(t, color)
  return text{ t, size = "sm", color = color or "text-muted", wrap = true }
end

local function head(t)
  return text{ t, size = "caption", weight = "semibold", color = "text-muted" }
end

local function lens_box(ui, open)
  if not open then
    return row{ w = "full", spacer{}, button{ "Write a lens…", key = "codemap-lens-open", icon = "plus", on = "lens_toggle", variant = "ghost",
      help = "Retell the map for one reader (a product owner, a new joiner)" } }
  end
  return column{ gap = 4, w = "full", pad = 8, radius = "sm", border = "border",
    row{ w = "full", text{ "Write a lens", size = "sm", weight = "medium" }, spacer{},
      button{ icon = "close", key = "codemap-lens-close", on = "lens_toggle", variant = "ghost", help = "Close" } },
    note("Retell the map for one reader, in a background session of this sandbox's agent."),
    input{ id = "codemap-lens", value = ui.lens or "", placeholder = "a product owner: what each part delivers, for whom, what it costs to change", on = "lens_typed" },
    row{ gap = 8, w = "full",
      text{ (ui.lens_left or 400) .. " characters left", size = "xs", color = (ui.lens_left or 400) < 0 and "red" or "text-dim" },
      spacer{},
      button{ "Write the lens", key = "codemap-lens-send", icon = "send", on = "lens", arg = ui.sandbox,
        disabled = ui.writing or not ui.running or not nz(ui.lens) or (ui.lens_left or 0) < 0,
        help = ui.running and "Runs /codemap-lens in the background" or "Start the sandbox first" },
    },
  }
end

local function empty(ui)
  local run = ui.run
  return column{ gap = 8, pad = 12, w = "full",
    text{ "No code map in this sandbox yet.", weight = "semibold" },
    note("A map is a few linked Markdown files under .sbxw-artifacts/codemap/ saying what the project does and why, and where each idea lives."),
    run and nz(run.label) and note(run.label, run.phase == "failed" and "red" or "amber") or false,
    row{ gap = 8,
      button{ ui.writing and "Writing the map…" or "Generate code map", key = "codemap-generate", icon = "sparkle", variant = "primary",
        on = "generate", arg = ui.sandbox, disabled = ui.writing or not ui.running,
        help = ui.running and "Runs /codemap in a background session: your pane stays yours" or "Start the sandbox first" },
    },
  }
end

local function side(ui, d)
  local kids = { gap = 4 }
  kids[#kids + 1] = head("Referenced by (" .. #(d.backlinks or {}) .. ")")
  if #(d.backlinks or {}) == 0 then
    kids[#kids + 1] = note("Nothing links here yet.")
  end
  for i, b in ipairs(d.backlinks or {}) do
    kids[#kids + 1] = row{ gap = 8, w = "full", px = 4, py = 2, radius = "xs", key = "codemap-back-" .. i, on = "doc", arg = { doc = b.from, anchor = b.anchor },
      text{ b.from, size = "sm", color = "blue" }, nz(b.section) and text{ b.section, size = "xs", color = "text-dim", truncate = true, grow = 1 } or false }
  end
  kids[#kids + 1] = spacer{ h = 4 }
  kids[#kids + 1] = head("Outline")
  for i, o in ipairs(d.outline or {}) do
    kids[#kids + 1] = row{ w = "full", px = 4 + 10 * ((o.level or 1) - 1), py = 2, radius = "xs", key = "codemap-outline-" .. o.id,
      on = "doc", arg = { doc = d.key, anchor = o.id }, text{ o.title, size = "sm", truncate = true } }
  end
  if #(d.code or {}) > 0 then
    kids[#kids + 1] = spacer{ h = 4 }
    kids[#kids + 1] = head("Code referenced")
    for i, c in ipairs(d.code) do
      kids[#kids + 1] = row{ w = "full", px = 4, py = 2, radius = "xs", key = "codemap-code-" .. i, on = "copy", arg = c, help = "Copy the path",
        icon{ "copy", size = 12, color = "text-dim" }, text{ c, size = "xs", mono = true, truncate = true, grow = 1 } }
    end
  end
  return scroll(kids)
end

local function doc_view(ui, m)
  local d = m.doc
  if not d then return note("Pick a file of the map.") end
  return column{ gap = 8, grow = 1,
    row{ gap = 8, w = "full",
      text{ ".sbxw-artifacts/" .. (d.path or "") .. (d.section and ("  ›  " .. d.section.title) or ""), size = "xs", mono = true, color = "text-dim", truncate = true, grow = 1 },
      d.section and button{ "Whole file", key = "codemap-whole", on = "doc", arg = { doc = d.key }, variant = "ghost" } or false,
    },
    split{ id = "codemap-split", ratio = 0.68,
      scroll{ markdown{ d.markdown or "", on = "link", size = "sm" } },
      side(ui, d) },
  }
end

local function results(m)
  local list = { gap = 2 }
  for i, d in ipairs(m.docs or {}) do
    list[#list + 1] = row{ gap = 8, w = "full", px = 8, py = 4, radius = "xs", key = "codemap-hit-" .. d.key, on = "doc", arg = { doc = d.key },
      text{ d.title, size = "sm", truncate = true, grow = 1 }, text{ d.key, size = "xs", color = "text-dim" } }
  end
  if #(m.docs or {}) == 0 then list[#list + 1] = note("No file of the map holds those words.") end
  return scroll(list)
end

return {
  init = function() return { lens_open = false } end,
  render = function(state, data)
    local ui = data.codemap_ui
    if not ui or not ui.sandbox then
      return column{ pad = 12, note("Open a code map from a sandbox's Project panel.") }
    end
    local m = ui.map or {}
    local maps = {}
    for _, x in ipairs(ui.maps or {}) do maps[#maps + 1] = { id = x.dir, label = x.label .. " · " .. (x.files or 0) .. " files" } end
    local docs = {}
    for _, d in ipairs(m.docs or {}) do docs[#docs + 1] = { id = d.key, label = d.title } end
    local top = row{ gap = 8, w = "full", badge{ ui.sandbox, tone = "blue" },
      #maps > 1 and select{ id = "codemap-maps", items = maps, value = ui.root, on = "map", arg = ui.sandbox } or text{ nz(ui.root) and (".sbxw-artifacts/" .. ui.root .. "/") or "", size = "xs", mono = true, color = "text-dim", truncate = true },
      spacer{},
      ui.diagram and button{ "Diagram", key = "codemap-diagram", icon = "link", on = "diagram", variant = "ghost",
        help = "An HTML page the app does not draw: saved, then opened in your browser (the app asks first)" } or false,
    }
    local body
    if ui.loading then
      body = note("Reading the map…")
    elseif (m.count or 0) == 0 then
      body = empty(ui)
    else
      body = column{ gap = 8, grow = 1,
        row{ gap = 8, w = "full",
          select{ id = "codemap-docs", items = docs, value = m.doc and m.doc.key or nil, placeholder = "File", on = "pick" },
          input{ id = "codemap-filter", value = ui.filter or "", placeholder = "Search the map", on = "filter", grow = 1 },
        },
        row{ gap = 8, w = "full",
          text{ (m.count or 0) .. " files", size = "xs", color = "text-muted" },
          (m.broken or 0) > 0 and badge{ m.broken .. " broken links", tone = "red", help = "Links to a file or section that is not there: the map drifted from the code" } or false,
          ui.run and nz(ui.run.label) and text{ ui.run.label, size = "xs", color = ui.run.phase == "failed" and "red" or "amber" } or false,
        },
        nz(ui.filter) and results(m) or doc_view(ui, m),
      }
    end
    return column{ h = "full", gap = 8, pad = 12,
      top,
      nz(ui.error) and note(ui.error, "red") or false,
      body,
      (m.count or 0) > 0 and lens_box(ui, state.lens_open or (ui.lens or "") ~= "") or false,
    }
  end,
  actions = {
    map = function(_, sb, dir) return intent("codemap_open", { sandbox = sb, map = dir }) end,
    pick = function(_, _, key) return intent("codemap_doc", { doc = key }) end,
    doc = function(_, a)
      if a.anchor and a.anchor ~= "" then return intent("codemap_doc", { doc = a.doc, anchor = a.anchor }) end
      return intent("codemap_doc", { doc = a.doc })
    end,
    link = function(_, _, url) return intent("codemap_link", { url = url }) end,
    filter = function(_, _, t) return intent("codemap_filter", { text = t or "" }) end,
    copy = function(_, c) return intent("copy_text", { text = c }) end,
    generate = function(_, sb) return intent("codemap_generate", { sandbox = sb }) end,
    lens_typed = function(_, _, t) return intent("codemap_lens_set", { text = t or "" }) end,
    lens = function(state, sb) state.lens_open = false return intent("codemap_lens", { sandbox = sb }) end,
    lens_toggle = function(state) state.lens_open = not state.lens_open end,
    diagram = function() return intent("codemap_diagram", {}) end,
  },
}
