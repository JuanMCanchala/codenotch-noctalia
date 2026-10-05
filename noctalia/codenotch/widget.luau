-- Barra: un icono por proveedor con una barrita de uso debajo. El icono de
-- Claude late mientras trabaja y parpadea en ámbar cuando te espera.
-- Los porcentajes viven en el panel (clic) y en el tooltip.
local fmt = require("./lib/format.luau")

local data = noctalia.state.get("codenotch")
local frame = 0
local PULSE = { 1.0, 0.75, 0.5, 0.75 }
local CODENOTCH = noctalia.expandPath("~/.local/bin/codenotch")
local showPercent = noctalia.getConfig("show_percent") == true

local function animating()
  for _, p in ipairs(data and data.providers or {}) do
    local a = p.activity
    if a and (a.state == "busy" or a.state == "waiting") then
      return true
    end
  end
  return false
end

-- Color y opacidad del icono según la actividad de Claude.
local function iconStyle(p)
  local a = p.activity
  local state = a and a.state
  if state == "busy" then
    return "primary", PULSE[frame % 4 + 1]
  elseif state == "waiting" then
    return fmt.AMBER, frame % 2 == 0 and 1 or 0.35
  elseif state == "done" then
    return "primary", 1
  end
  return nil, 1
end

local function tooltipRows()
  local rows = {}
  for _, p in ipairs(data.providers) do
    local parts = {}
    for _, w in ipairs(p.windows or {}) do
      table.insert(parts, fmt.pct(w.used))
    end
    local value = #parts > 0 and table.concat(parts, " / ") or "–"
    local a = p.activity
    if a and a.state ~= "idle" then
      value = value .. " · " .. fmt.ACTIVITY[a.state]
    elseif p.error then
      value = value .. " · sin actualizar"
    end
    local key = p.kind == "claude" and ("Claude " .. p.label) or p.label
    table.insert(rows, { key = key, value = value })
  end
  return rows
end

local function providerItem(p)
  local w = p.windows and p.windows[1]
  local used = w and w.used or 0
  local usage = w and fmt.usageColor(w.used)
  local color, opacity = iconStyle(p)
  -- Sin actividad, el icono avisa del nivel de uso.
  color = color or usage

  local icon = ui.column({ gap = 2, align = "center" }, {
    ui.glyph({ name = fmt.KIND_GLYPH[p.kind] or "gauge", size = 14, color = color, opacity = opacity }),
    ui.progress({
      progress = used,
      fill = usage or "on_surface/0.75",
      track = "on_surface/0.15",
      width = 14,
      height = 2,
      radius = 1,
    }),
  })
  local children = { icon }
  if showPercent and w then
    table.insert(children, ui.label({ text = fmt.pct(w.used), fontWeight = "bold", color = usage }))
  end
  return ui.row({ gap = 4, align = "center", opacity = p.stale and 0.45 or 1 }, children)
end

local function render()
  local container = barWidget.isVertical() and ui.column or ui.row
  if not data or data.offline or #(data.providers or {}) == 0 then
    barWidget.render(container({ align = "center" }, {
      ui.glyph({ name = "gauge", size = 14, color = "on_surface/0.5" }),
    }))
    barWidget.setTooltip(
      (data and data.offline) and "codenotch: el daemon no está corriendo" or "codenotch: sin proveedores"
    )
    return
  end

  local items = {}
  for _, p in ipairs(data.providers) do
    table.insert(items, providerItem(p))
  end
  barWidget.render(container({ gap = 10, align = "center" }, items))
  barWidget.setTooltip(tooltipRows())
end

local function reschedule()
  -- Animación solo mientras hay algo que animar.
  noctalia.setUpdateInterval(animating() and 600 or 60000)
end

function update()
  frame = frame + 1
  render()
  reschedule()
end

noctalia.state.watch("codenotch", function(value)
  data = value
  render()
  reschedule()
end)

function onClick()
  noctalia.togglePanel("juanmcanchala/codenotch:panel")
end

function onRightClick()
  noctalia.runAsync({ CODENOTCH, "refresh" })
end

render()
reschedule()
