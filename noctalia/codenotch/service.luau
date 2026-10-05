-- Publica el estado del daemon para el widget y el panel.
--
-- El daemon avisa por IPC (`noctalia msg plugin juanmcanchala/codenotch:service all update`)
-- cada vez que reescribe el archivo; el sondeo lento de abajo solo cubre un
-- aviso perdido y nota si el daemon dejó de correr.

local runtime = noctalia.getenv("XDG_RUNTIME_DIR") or "/run/user/1000"
local STATE = runtime .. "/codenotch/state.json"

local lastMtime = nil
local data = nil
local running = true

local function publish()
  if data then
    data.offline = not running
    noctalia.state.set("codenotch", data)
  else
    noctalia.state.set("codenotch", { offline = true, providers = {} })
  end
end

local function load(force)
  local info = noctalia.fileInfo(STATE)
  if not info then
    data = nil
    lastMtime = nil
    publish()
    return
  end
  if not force and info.mtime == lastMtime then
    return
  end
  lastMtime = info.mtime
  noctalia.readFileAsync(STATE, function(contents)
    local decoded = contents and noctalia.json.decode(contents)
    if type(decoded) == "table" and decoded.providers then
      data = decoded
      publish()
    end
  end)
end

local function checkDaemon()
  noctalia.processMatches(function(matched)
    if matched ~= running then
      running = matched
      publish()
    end
  end, "codenotch")
end

noctalia.setUpdateInterval(15000)

function update()
  load(false)
  checkDaemon()
end

function onIpc(event)
  if event == "update" then
    running = true
    load(true)
  end
end

load(true)
checkDaemon()
