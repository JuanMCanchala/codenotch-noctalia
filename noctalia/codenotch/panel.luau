-- Panel: cada proveedor con el consumo de sus ventanas de límite y cuándo reinician.
local fmt = require("./lib/format.luau")

local data = noctalia.state.get("codenotch")
local isOpen = false
local CODENOTCH = noctalia.expandPath("~/.local/bin/codenotch")

local function dim(text, size)
  return ui.label({ text = text, fontSize = size or 11, color = "on_surface/0.55" })
end

local function windowNode(w)
  local color = fmt.usageColor(w.used)
  local children = {
    ui.row({ align = "center" }, {
      ui.label({ text = w.label, fontSize = 12, color = "on_surface/0.8" }),
      ui.spacer({ flexGrow = 1 }),
      ui.label({ text = fmt.pct(w.used), fontSize = 12, fontWeight = "bold", color = color or "on_surface" }),
    }),
    ui.progress({ progress = w.used, fill = color or "primary", track = "on_surface/0.1", height = 6, radius = 3 }),
  }
  local sub = {}
  if w.detail then
    table.insert(sub, w.detail)
  end
  if w.resets_at then
    table.insert(sub, fmt.untilText(w.resets_at) .. " (" .. fmt.clock(w.resets_at) .. ")")
  end
  if #sub > 0 then
    table.insert(children, dim(table.concat(sub, " · ")))
  end
  return ui.column({ gap = 4 }, children)
end

local function providerNode(p)
  local header = {
    ui.glyph({ name = fmt.KIND_GLYPH[p.kind] or "gauge", size = 15, color = "primary" }),
    ui.label({ text = fmt.title(p), fontSize = 13, fontWeight = "bold" }),
  }
  if p.plan then
    table.insert(header, dim(p.plan))
  end
  table.insert(header, ui.spacer({ flexGrow = 1 }))
  if p.stale and p.fetched_at then
    table.insert(header, dim(fmt.ago(p.fetched_at)))
  end

  local children = { ui.row({ gap = 6, align = "center" }, header) }
  for _, w in ipairs(p.windows or {}) do
    table.insert(children, windowNode(w))
  end
  if #(p.windows or {}) == 0 and not p.error then
    table.insert(children, dim("sin lectura todavía"))
  end
  if p.error then
    table.insert(children, ui.label({ text = p.error, fontSize = 11, color = "error" }))
  end
  return ui.column({ gap = 8, opacity = p.stale and 0.75 or 1 }, children)
end

local function render()
  if not isOpen then
    return
  end
  local body = {
    ui.row({ align = "center" }, {
      ui.label({ text = "Uso de IA", fontSize = 15, fontWeight = "bold" }),
      ui.spacer({ flexGrow = 1 }),
      ui.button({ glyph = "refresh", variant = "ghost", controlSize = "sm", tooltip = "Consultar ahora", onClick = "refresh" }),
    }),
  }
  if not data or data.offline then
    table.insert(body, dim("El daemon no está corriendo:\nsystemctl --user start codenotch", 12))
  end
  for i, p in ipairs(data and data.providers or {}) do
    if i > 1 then
      table.insert(body, ui.separator({ color = "on_surface/0.08" }))
    end
    table.insert(body, providerNode(p))
  end
  if data and data.generated_at then
    table.insert(body, dim("Actualizado " .. fmt.ago(data.generated_at) .. " · clic derecho en la barra para consultar"))
  end
  panel.render(ui.scroll({ padding = 16, gap = 14 }, body))
end

function refresh()
  noctalia.runAsync({ CODENOTCH, "refresh" })
end

-- Las cuentas atrás ("reinicia en 13 min") van contra el reloj, no contra la
-- última lectura: se repintan mientras el panel está abierto.
noctalia.setUpdateInterval(30000)

function update()
  render()
end

function onOpen()
  isOpen = true
  data = noctalia.state.get("codenotch")
  render()
end

function onClose()
  isOpen = false
end

noctalia.state.watch("codenotch", function(value)
  data = value
  render()
end)
