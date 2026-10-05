-- Formato compartido por el widget y el panel.
local M = {}

M.AMBER = "#f5a524"

M.ACTIVITY = {
  busy = "trabajando",
  waiting = "te espera",
  done = "terminó",
  idle = "inactiva",
}

M.KIND_GLYPH = {
  claude = "asterisk",
  codex = "brand-openai",
  kiro = "ghost",
}

function M.pct(used)
  return string.format("%d%%", math.floor((used or 0) * 100 + 0.5))
end

-- nil = color normal del texto.
function M.usageColor(used)
  if not used then
    return nil
  end
  if used >= 0.9 then
    return "error"
  end
  if used >= 0.7 then
    return M.AMBER
  end
  return nil
end

function M.title(p)
  if p.kind == "claude" then
    return "Claude · " .. p.label
  end
  return p.label
end

function M.untilText(t)
  local d = t - os.time()
  if d <= 0 then
    return "reinicia ya"
  end
  local days, h, m = d // 86400, (d % 86400) // 3600, (d % 3600) // 60
  if days > 0 then
    return string.format("reinicia en %d d %d h", days, h)
  end
  if h > 0 then
    return string.format("reinicia en %d h %d min", h, m)
  end
  return string.format("reinicia en %d min", math.max(m, 1))
end

function M.clock(t)
  if t - os.time() < 86400 then
    return os.date("%H:%M", t)
  end
  return os.date("%d/%m %H:%M", t)
end

function M.ago(t)
  local d = os.time() - t
  if d < 60 then
    return "hace un momento"
  end
  if d < 3600 then
    return string.format("hace %d min", d // 60)
  end
  return string.format("hace %d h", d // 3600)
end

return M
