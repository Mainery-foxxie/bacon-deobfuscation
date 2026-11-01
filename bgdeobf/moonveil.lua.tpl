-- bgdeobf: MoonVeil trace harness (see moonveil.py); Python fills in the settings table and the script below.
-- The protected script runs against a recording fake Roblox environment.  Every call it makes is written as one
-- tab-separated line:  B/E = a thread starts/ends, C = call, S = assignment, N = note, I/X = loop over a fake list,
-- T = trace finished or cut.  Nothing here touches the network or the file system.
-- The script is defined first so that none of the harness locals below are visible to it (it only sees `env`).
local __M = function(...)
@@SRC@@
end
local CFG = @@CFG@@
local real = getfenv()
local out = print
local tostring, tonumber, type, pairs, ipairs, select, pcall, error, next = tostring, tonumber, type, pairs, ipairs, select, pcall, error, next
local rawequal, rawset, rawget, setmetatable = rawequal, rawset, rawget, setmetatable
local sfmt, ssub, sgsub, tconcat, tsort = string.format, string.sub, string.gsub, table.concat, table.sort
local mfloor, mhuge, mmin, mabs = math.floor, math.huge, math.min, math.abs
local cocreate, coresume, costatus, coyield = coroutine.create, coroutine.resume, coroutine.status, coroutine.yield

local KEYWORDS = {}
for w in ("and break do else elseif end false for function if in local nil not or repeat return then true until while"):gmatch("%a+") do
    KEYWORDS[w] = true
end
local function isid(k)
    return type(k) == "string" and k:find("^[%a_][%w_]*$") ~= nil and not KEYWORDS[k]
end

-- ------------------------------------------------------------------------------------------------ output
local N, STOP, MUTE = 0, false, 0
local WS = nil -- the thread being run: {n = waits, ev = events, begin = {bid, labels, probe}}
local function raw(...)
    if MUTE > 0 then return end
    out(tconcat({...}, "\t"))
end
local function flush_begin()
    local b = WS and WS.begin
    if b then
        WS.begin = nil
        raw("B", b[1], b[2], b[3])
    end
end
local function emit(...)
    if STOP or MUTE > 0 then return end
    N = N + 1
    if N > CFG.maxevents then
        STOP = true
        raw("T", "cap")
        return
    end
    flush_begin()
    if WS then WS.ev = WS.ev + 1 end
    raw(...)
end

-- ------------------------------------------------------------------------------------------------ values as Lua text
local proxies = setmetatable({}, {__mode = "k"})
local FUNCS, fids, nextf = {}, {}, 0
local function fid(f)
    local n = fids[f]
    if not n then
        nextf = nextf + 1
        n = nextf
        fids[f], FUNCS[n] = n, f
    end
    return n
end
local FNAME = {} -- the functions of the fake environment and of the standard library, by name
local strs, strset = {}, {}
local function note_str(s)
    if #s >= 2 and #s <= 40 and not strset[s] and #strs < 300 and not s:find("%c") then
        strset[s] = true
        strs[#strs + 1] = s
    end
end
local function q(s)
    local cut = nil
    if #s > CFG.maxstr then
        cut = #s - CFG.maxstr
        s = ssub(s, 1, CFG.maxstr)
    end
    s = (sgsub(s, '[%c"\\]', function(c)
        if c == '"' then return '\\"' end
        if c == "\\" then return "\\\\" end
        if c == "\n" then return "\\n" end
        if c == "\t" then return "\\t" end
        if c == "\r" then return "\\r" end
        return sfmt("\\%03d", c:byte())
    end))
    s = '"' .. s .. '"'
    if cut then s = s .. "--[[+" .. cut .. " more chars]]" end
    return s
end
local function num(x)
    if x ~= x then return "(0/0)" end
    if x == mhuge then return "math.huge" end
    if x == -mhuge then return "-math.huge" end
    if x == mfloor(x) and mabs(x) < 1e15 then return sfmt("%d", x) end
    return sfmt("%.10g", x)
end
local fmt
local function prio(k)
    local s = tostring(k):lower()
    if s == "text" or s == "title" or s == "name" then return 0 end
    if s:find("callback") or s == "func" or s == "function" then return 2 end
    return 1
end
local function tfmt(t, d)
    local info = proxies[t]
    if info then return info.expr end
    if d > 2 then return "{--[[nested]]}" end
    local parts, n = {}, #t
    for i = 1, mmin(n, 20) do parts[#parts + 1] = fmt(t[i], d + 1) end
    local named = {}
    for k in pairs(t) do
        if not (type(k) == "number" and k >= 1 and k <= n and k == mfloor(k)) then named[#named + 1] = k end
    end
    tsort(named, function(a, b)
        local pa, pb = prio(a), prio(b)
        if pa ~= pb then return pa < pb end
        return tostring(a) < tostring(b)
    end)
    for i = 1, mmin(#named, 20) do
        local k = named[i]
        local v = fmt(t[k], d + 1)
        if isid(k) then parts[#parts + 1] = k .. " = " .. v else parts[#parts + 1] = "[" .. fmt(k, 3) .. "] = " .. v end
    end
    if n > 20 or #named > 20 then parts[#parts] = parts[#parts] .. " --[[+more]]" end
    return "{" .. tconcat(parts, ", ") .. "}"
end
fmt = function(v, d)
    d = d or 0
    local t = type(v)
    if t == "nil" then return "nil" end
    if t == "boolean" then return tostring(v) end
    if t == "number" then return num(v) end
    if t == "string" then
        note_str(v)
        return q(v)
    end
    if t == "function" then return FNAME[v] or ("@F" .. fid(v)) end
    if t == "table" then return tfmt(v, d) end
    return "nil--[[" .. t .. "]]"
end
local function plain(e)
    return (sgsub(e, "@%u%d+", "?"))
end
local function idxexpr(pe, k)
    if isid(k) then return pe .. "." .. k end
    return pe .. "[" .. fmt(k, 3) .. "]"
end
local function ex(x)
    local i = proxies[x]
    if i then return i.expr end
    return fmt(x, 3)
end

-- ------------------------------------------------------------------------------------------------ threads
local CLOCK = 1000
local function park()
    pcall(coyield) -- the thread is parked for good; the harness never resumes it
    error("__PARKED", 0)
end
local function dowait(t)
    t = tonumber(t) or 0.03
    if STOP or (WS and WS.n >= CFG.waitcap) then park() end
    if WS then WS.n = WS.n + 1 end
    CLOCK = CLOCK + t + 0.01
    return t
end
local function runco(f, bid, lab, probe, always, ...)
    local saved = WS
    WS = {n = 0, ev = 0, begin = {bid, lab, probe}}
    if always then flush_begin() end
    local co = cocreate(f)
    local ok, err = coresume(co, ...)
    local status = "ok"
    if not ok then
        err = tostring(err)
        if err:find("__PARKED", 1, true) then
            status = "cut"
        else
            err = (sgsub(err, "%c", " "))
            err = (sgsub(err, "^[^:]*:%d+: ", ""))
            status = "err:" .. ssub(err, 1, 200)
        end
    elseif costatus(co) ~= "dead" then
        status = "cut"
    end
    local ev = WS.ev
    if WS.begin == nil then raw("E", bid, status) end
    WS = saved
    return ev, status
end

-- ------------------------------------------------------------------------------------------------ callbacks
local PENDING, REG, RAN = {}, {}, {}
local FINDARGS, findseen = {}, {}
local function reg(f, kind, label)
    if FNAME[f] or REG[f] or RAN[f] or MUTE > 0 then return end
    REG[f] = true
    PENDING[#PENDING + 1] = {f = f, kind = kind, label = label}
end
local FRAME = {Heartbeat = 1, RenderStepped = 1, Stepped = 1, PreRender = 1, PreSimulation = 1, PostSimulation = 1,
               PreAnimation = 1}
local function kind_of(info)
    local k = info.key
    if k == "Connect" or k == "Once" or k == "ConnectParallel" then
        local p = info.parent and proxies[info.parent]
        if p and FRAME[p.key] then return "frame" end
        return "event"
    end
    local lk = tostring(k or ""):lower()
    if lk:find("toggle") or lk:find("checkbox") or lk:find("switch") then return "toggle" end
    if lk:find("slider") then return "slider" end
    if lk:find("textbox") or lk:find("input") then return "text" end
    if lk:find("dropdown") or lk:find("picker") then return "choice" end
    if lk:find("keybind") then return "key" end
    return "plain"
end
local function scan_callbacks(info, args, from, to)
    local kind = kind_of(info)
    for i = from, to do
        local v = args[i]
        if type(v) == "function" then
            reg(v, kind, info.expr)
        elseif type(v) == "table" and not proxies[v] then
            for _, x in pairs(v) do
                if type(x) == "function" then
                    reg(x, kind, info.expr)
                elseif type(x) == "table" and not proxies[x] then
                    for _, y in pairs(x) do
                        if type(y) == "function" then reg(y, kind, info.expr) end
                    end
                end
            end
        end
    end
end

-- ------------------------------------------------------------------------------------------------ fake objects
local DIST, DIST_READ, LASTNOTE = 0, false, nil
local RID, EID = 0, 0
local META = {}
local function mk(expr, parent, key, fields)
    local p = setmetatable({}, META)
    proxies[p] = {expr = expr, parent = parent, key = key, kids = {}, fields = fields}
    return p
end
local function note(s)
    if s ~= LASTNOTE then
        LASTNOTE = s
        emit("N", s)
    end
end
META.__index = function(self, k)
    if k == nil or k ~= k then return nil end
    local info = proxies[self]
    local f = info.fields
    if f then
        local v = f[k]
        if v ~= nil then return v end
    end
    if k == "Magnitude" then
        DIST_READ = true
        note("reads a distance: " .. info.expr .. ".Magnitude")
        return DIST
    end
    local c = info.kids[k]
    if c == nil then
        c = mk(idxexpr(info.expr, k), self, k)
        info.kids[k] = c
    end
    return c
end
META.__newindex = function(self, k, v)
    local info = proxies[self]
    local target = idxexpr(info.expr, k)
    local text = fmt(v, 0)
    if type(v) == "function" then reg(v, "plain", target) end
    emit("S", target, text)
end
local FINDERS = {FindFirstChild = 1, WaitForChild = 1, FindFirstDescendant = 1, FindFirstAncestor = 1,
                 FindFirstChildOfClass = 1, FindFirstChildWhichIsA = 1}
META.__call = function(self, ...)
    local info = proxies[self]
    local n = select("#", ...)
    local args = {...}
    local key = info.key
    local start, callee = 1, info.expr
    local par = info.parent and proxies[info.parent]
    if par and n >= 1 and rawequal(args[1], info.parent) and isid(key) then
        callee, start = par.expr .. ":" .. key, 2
    end
    local parts = {}
    local last = mmin(n, start + 11)
    for i = start, last do parts[#parts + 1] = fmt(args[i], 0) end
    if n > last then parts[#parts] = parts[#parts] .. " --[[+more arguments]]" end
    if FINDERS[key] and type(args[start]) == "string" and not findseen[args[start]] then
        findseen[args[start]] = true
        FINDARGS[#FINDARGS + 1] = args[start]
    end
    scan_callbacks(info, args, start, last)
    RID = RID + 1
    local rid = RID
    emit("C", tostring(rid), callee, tconcat(parts, ", "))
    if key == "Wait" then return dowait(0.03) end
    return mk("@R" .. rid)
end
local function arith(op)
    return function(a, b) return mk("(" .. ex(a) .. " " .. op .. " " .. ex(b) .. ")") end
end
META.__add, META.__sub, META.__mul, META.__div = arith("+"), arith("-"), arith("*"), arith("/")
META.__mod, META.__pow, META.__idiv = arith("%"), arith("^"), arith("//")
META.__unm = function(a) return mk("(-" .. ex(a) .. ")") end
local function plainv(x)
    local i = proxies[x]
    if i then return "{" .. i.expr .. "}" end
    return tostring(x)
end
META.__concat = function(a, b) return plainv(a) .. plainv(b) end
META.__tostring = function(a) return plainv(a) end
META.__len = function() return 0 end
META.__lt = function() return false end
META.__le = function() return false end
local function iter(self)
    local info = proxies[self]
    EID = EID + 1
    local eid, done = EID, false
    emit("I", tostring(eid), info.expr)
    return function()
        if done then
            emit("X", tostring(eid))
            return nil
        end
        done = true
        return 1, mk("@E" .. eid)
    end
end
META.__iter = iter

-- ------------------------------------------------------------------------------------------------ the environment
local function envcall(callee, ...)
    local n = select("#", ...)
    local a = {...}
    local parts = {}
    for i = 1, mmin(n, 12) do parts[i] = fmt(a[i], 0) end
    RID = RID + 1
    emit("C", tostring(RID), callee, tconcat(parts, ", "))
end
local DEPTH, TOTAL, SPAWNED = 0, 0, {}
local function spawnrun(f, ...)
    if type(f) ~= "function" or FNAME[f] then return end
    RAN[f] = true
    if DEPTH >= CFG.maxdepth or TOTAL >= CFG.maxruns then return end
    local c = (SPAWNED[f] or 0) + 1
    SPAWNED[f] = c
    DEPTH, TOTAL = DEPTH + 1, TOTAL + 1
    if c > 2 then MUTE = MUTE + 1 end
    runco(f, "F" .. fid(f), "kind=spawn", nil, true, ...)
    if c > 2 then MUTE = MUTE - 1 end
    DEPTH = DEPTH - 1
end
local function mkstore(name)
    return setmetatable({}, {__newindex = function(t, k, v)
        local target = idxexpr(name, k)
        local text = fmt(v, 0)
        if type(v) == "function" then reg(v, "plain", target) end
        emit("S", target, text)
        rawset(t, k, v)
    end})
end
local GENV = mkstore("getgenv()")
local env = {}
for _, k in ipairs({"type", "pairs", "ipairs", "next", "select", "unpack", "tostring", "tonumber", "pcall", "xpcall",
                    "error", "assert", "setmetatable", "getmetatable", "rawget", "rawset", "rawequal", "rawlen", "string",
                    "table", "math", "bit32", "coroutine", "utf8", "buffer", "debug", "_VERSION"}) do
    env[k] = real[k]
end
env.os = {clock = function() CLOCK = CLOCK + 0.05 return CLOCK end, time = function() return 1700000000 + mfloor(CLOCK) end,
          date = os.date}
env.tick = function() CLOCK = CLOCK + 0.05 return CLOCK end
env.time = env.tick
env.typeof = function(v)
    if proxies[v] then return "Instance" end
    return typeof(v)
end
env.pairs = function(t)
    if proxies[t] then return iter(t) end
    return pairs(t)
end
env.ipairs = function(t)
    if proxies[t] then return iter(t) end
    return ipairs(t)
end
local NEXTLOOP = setmetatable({}, {__mode = "k"})
env.next = function(t, k)
    if proxies[t] then
        if k == nil then
            EID = EID + 1
            NEXTLOOP[t] = EID
            emit("I", tostring(EID), proxies[t].expr)
            return 1, mk("@E" .. EID)
        end
        local eid = NEXTLOOP[t]
        if eid then
            NEXTLOOP[t] = nil
            emit("X", tostring(eid))
        end
        return nil
    end
    return next(t, k)
end
env.getfenv = function() return env end
env.setfenv = function(f) return f end
env.getgenv = function() return GENV end
env._G = mkstore("_G")
env.shared = mkstore("shared")
env.print = function(...) envcall("print", ...) end
env.warn = function(...) envcall("warn", ...) end
local function waitev(name, t)
    if t == nil then envcall(name) else envcall(name, t) end
    return dowait(t)
end
env.wait = function(t) return waitev("wait", t) end
env.task = {
    wait = function(t) return waitev("task.wait", t) end,
    spawn = function(f, ...)
        envcall("task.spawn", f, ...)
        spawnrun(f, ...)
        return mk("task")
    end,
    defer = function(f, ...)
        envcall("task.defer", f, ...)
        spawnrun(f, ...)
        return mk("task")
    end,
    delay = function(t, f, ...)
        envcall("task.delay", t, f, ...)
        spawnrun(f, ...)
        return mk("task")
    end,
    cancel = function() end,
}
env.spawn = function(f, ...)
    envcall("spawn", f, ...)
    spawnrun(f, ...)
end
env.delay = function(t, f, ...)
    envcall("delay", t, f, ...)
    spawnrun(f, ...)
end
for k, v in pairs(env) do
    if type(v) == "function" then FNAME[v] = k end
end
for _, lib in ipairs({"string", "table", "math", "bit32", "coroutine", "utf8", "debug", "task", "os"}) do
    local t = env[lib]
    if type(t) == "table" then
        for k, v in pairs(t) do
            if type(v) == "function" and not FNAME[v] then FNAME[v] = lib .. "." .. k end
        end
    end
end
local GREAD = {}
setmetatable(env, {__index = function(t, k)
    if type(k) ~= "string" then return nil end
    local v = mk(k)
    rawset(t, k, v)
    GREAD[#GREAD + 1] = k
    return v
end})

setfenv(__M, env)

-- ------------------------------------------------------------------------------------------------ run
local PARAMS = {toggle = "enabled", slider = "value", text = "text", choice = "choice", key = "key", event = "inst",
                frame = "dt"}
local function distruns(f, bid, base, probe, always, ...)
    DIST, DIST_READ = 0, false
    local ev, st = runco(f, bid, base .. ";dist=near", probe, always, ...)
    if DIST_READ then
        DIST, DIST_READ = CFG.far, false
        runco(f, bid, base .. ";dist=far", probe, always, ...)
    end
    DIST, DIST_READ = 0, false
    return ev, st
end
local function exercise(e)
    local f, kind = e.f, e.kind
    local bid = "F" .. fid(f)
    local param = PARAMS[kind]
    local base = "kind=" .. kind .. (param and (";param=" .. param) or "")
    if kind == "toggle" then
        distruns(f, bid, base .. ";val=true", nil, true, true)
        runco(f, bid, base .. ";val=false", nil, true, false)
    elseif kind == "slider" then
        distruns(f, bid, base, nil, true, 50)
    elseif kind == "text" then
        distruns(f, bid, base, nil, true, "text")
    elseif kind == "frame" then
        distruns(f, bid, base, nil, true, 1 / 60)
    elseif kind == "event" then
        local ev = distruns(f, bid, base, nil, true, mk("inst"))
        if ev == 0 then
            -- the handler did nothing for an unknown object: try objects with the names the script looked for
            local cand, seen = {}, {}
            for _, s in ipairs(FINDARGS) do
                if not seen[s] then seen[s] = true cand[#cand + 1] = s end
            end
            for _, s in ipairs(strs) do
                if not seen[s] then seen[s] = true cand[#cand + 1] = s end
            end
            for i = 1, mmin(#cand, CFG.maxprobe) do
                local inst = mk("inst", nil, nil, {Name = cand[i]})
                runco(f, bid, base .. ";dist=near", q(cand[i]), false, inst)
            end
        end
    elseif param then
        distruns(f, bid, base, nil, true, mk(param))
    else
        distruns(f, bid, base, nil, true)
    end
end

runco(__M, "M", "kind=main", nil, true)
local i = 1
while i <= #PENDING and i <= CFG.maxcb and not STOP do
    exercise(PENDING[i])
    i = i + 1
end
for _, k in ipairs(GREAD) do
    if proxies[rawget(env, k)] then raw("G", k) end -- read, never assigned by the script
end
raw("T", "done")
