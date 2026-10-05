-- Formatting and translation shared by the widget and the panel.
--
-- The daemon sends data, not text: window ids and durations, amounts and
-- error codes. Everything a person reads is built here from translations/.
local M = {}

M.AMBER = "#f5a524"

M.KIND_GLYPH = {
  claude = "asterisk",
  codex = "brand-openai",
  kiro = "ghost",
}

-- `language = "auto"` uses Noctalia's own catalog lookup; a fixed language
-- reads that catalog directly, falling back to Noctalia's for missing keys.
local catalog = nil
local language = noctalia.getConfig("language") or "auto"
if language ~= "auto" then
  local raw = noctalia.readFile("translations/" .. language .. ".json")
  catalog = raw and noctalia.json.decode(raw) or nil
end

local function substitute(text, vars)
  return (text:gsub("{(%w+)}", function(name)
    local value = vars[name]
    if value == nil then
      return "{" .. name .. "}"
    end
    return tostring(value)
  end))
end

function M.t(key, vars)
  local text = catalog and catalog[key]
  if text then
    return vars and substitute(text, vars) or text
  end
  if vars then
    return substitute(noctalia.tr(key), vars)
  end
  return noctalia.tr(key)
end

function M.pct(used)
  return string.format("%d%%", math.floor((used or 0) * 100 + 0.5))
end

-- nil = the normal text color.
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

function M.activity(state)
  return M.t("activity." .. state)
end

function M.durationLabel(secs)
  if secs < 86400 then
    return M.t("window.hours", { n = (secs + 1800) // 3600 })
  end
  if secs >= 6 * 86400 and secs <= 8 * 86400 then
    return M.t("window.week")
  end
  if secs >= 27 * 86400 and secs <= 32 * 86400 then
    return M.t("window.month")
  end
  return M.t("window.days", { n = (secs + 43200) // 86400 })
end

local NAMED_WINDOWS = { session = true, week = true, extra = true, credits = true }

function M.windowLabel(w)
  local id = w.id or ""
  local base
  if NAMED_WINDOWS[id] then
    base = M.t("window." .. id)
  elseif id:sub(1, 5) == "week_" then
    local model = id:sub(6)
    base = M.t("window.week_model", { model = model:sub(1, 1):upper() .. model:sub(2) })
  elseif w.duration then
    base = M.durationLabel(w.duration)
  else
    base = w.label or M.t("window.generic")
  end
  if w.group then
    return w.group .. " · " .. base
  end
  return base
end

local function trimNumber(x)
  return (string.format("%.2f", x):gsub("%.?0+$", ""))
end

function M.amountText(a)
  if not a then
    return nil
  end
  if a.unit == "credits" then
    return M.t("amount.credits", { used = trimNumber(a.used), total = trimNumber(a.total) })
  end
  return M.t("amount.money", {
    used = string.format("%.2f", a.used),
    total = string.format("%.2f", a.total),
    unit = a.unit,
  })
end

function M.errorText(p)
  local code = p.error_code
  if code == "auth" then
    return M.t("error.auth." .. p.kind)
  elseif code == "rate_limited" or code == "network" then
    return M.t("error." .. code)
  end
  return p.error and M.t("error.other", { message = p.error }) or nil
end

function M.untilText(t)
  local d = t - os.time()
  if d <= 0 then
    return M.t("reset.now")
  end
  local days, h, m = d // 86400, (d % 86400) // 3600, (d % 3600) // 60
  if days > 0 then
    return M.t("reset.in_days", { d = days, h = h })
  end
  if h > 0 then
    return M.t("reset.in_hours", { h = h, m = m })
  end
  return M.t("reset.in_minutes", { m = math.max(m, 1) })
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
    return M.t("time.just_now")
  end
  if d < 3600 then
    return M.t("time.minutes_ago", { n = d // 60 })
  end
  return M.t("time.hours_ago", { n = d // 3600 })
end

return M
