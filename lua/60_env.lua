-- part 6: the global environment of the script: the Luau and Roblox globals, the executor functions, and a probe for names
-- that do not exist.  The script (and every loadstring payload) runs with this table as its environment.
do
    local ENV = {}
    rt.ENV = ENV
    local EX = rt.EX
    local emit, repr, quote = rt.emit, rt.repr, rt.quote
    local real = rt.real
    local newobj, defkind = rt.newobj, rt.defkind

    local function copy_table(t)
        local o = {}
        for k, v in next, t do o[k] = v end
        return o
    end

    -- ------------------------------------------------------------------------------------------ plain Luau
    for _, n in ipairs({'type', 'tostring', 'tonumber', 'pcall', 'xpcall', 'error', 'assert', 'select', 'next', 'unpack', 'rawget', 'rawset',
        'rawequal', 'rawlen', 'setmetatable', 'getmetatable', 'newproxy'}) do
        ENV[n] = real[n]
    end
    ENV._VERSION = 'Luau'
    ENV.typeof = rt.typeof

    -- iterating a Ghost works like iterating an empty table
    local real_pairs, real_ipairs = real.pairs, real.ipairs
    local function empty_iter() return nil end
    ENV.pairs = function(t)
        if KIND[t] == 'Ghost' then return empty_iter, t, nil end
        return real_pairs(t)
    end
    ENV.ipairs = function(t)
        if KIND[t] == 'Ghost' then return empty_iter, t, nil end
        return real_ipairs(t)
    end

    -- A Ghost stands for a value the sandbox does not know.  When a script hands one to a library function that wants a
    -- number, a string or a table, the Ghost becomes a harmless one (0, its own text, an empty table), so the script goes
    -- on instead of stopping on an error the real game would not have had.
    local CONV = {
        n = function() return 0 end,
        s = function(v) return tostring(v) end,
        t = function() return {} end,
    }
    -- only the first argument can be a Ghost (cheap: library calls like these are hot in a protected script)
    local function first(f, kind)
        local conv = CONV[kind]
        return function(a, ...)
            if type(a) == 'userdata' and KIND[a] == 'Ghost' then a = conv(a) end
            return f(a, ...)
        end
    end
    local function first_lib(lib, names, kind)
        for _, name in ipairs(names) do
            if type(lib[name]) == 'function' then lib[name] = first(lib[name], kind) end
        end
    end
    -- every argument can be a Ghost (the math library: all arguments are numbers)
    local function every(f)
        return function(...)
            local n = select('#', ...)
            local args
            for i = 1, n do
                local v = (select(i, ...))
                if type(v) == 'userdata' and KIND[v] == 'Ghost' then
                    args = args or {...}
                    args[i] = 0
                end
            end
            if args then return f(unpack(args, 1, n)) end
            return f(...)
        end
    end

    -- string.format knows what each %-spec wants: %s takes anything, the number specs get a number
    local lib_string = copy_table(real.string)
    local real_format = real.string.format
    local NUMSPEC = {d = true, i = true, u = true, c = true, x = true, X = true, o = true, e = true, E = true, f = true, g = true, G = true}
    lib_string.format = function(fmt, ...)
        local n = select('#', ...)
        if n == 0 or type(fmt) ~= 'string' then return real_format(fmt, ...) end
        local a = {...}
        local i = 0
        for conv in string.gmatch(fmt, '%%[%-%+ #0]*%d*%.?%d*([%a%%])') do
            if conv ~= '%' then
                i = i + 1
                local v = a[i]
                if type(v) == 'userdata' then
                    if conv == 's' then a[i] = tostring(v)
                    elseif KIND[v] == 'Ghost' and NUMSPEC[conv] then a[i] = 0 end
                end
            end
        end
        return real_format(fmt, unpack(a, 1, n))
    end
    ENV.string = lib_string
    first_lib(lib_string, {'lower', 'upper', 'len', 'reverse', 'find', 'match', 'gmatch', 'gsub', 'split'}, 's')
    do
        local real_rep = lib_string.rep
        lib_string.rep = function(s, n, sep)
            if type(s) == 'userdata' and KIND[s] == 'Ghost' then s = tostring(s) end
            if type(n) == 'userdata' and KIND[n] == 'Ghost' then n = 1 end
            if sep == nil then return real_rep(s, n) end
            return real_rep(s, n, sep)
        end
    end

    local lib_math = copy_table(real.math)
    local function rfloat() return (rt.rand_int(1073741824) - 1) / 1073741824 end
    local function one(v) if type(v) == 'userdata' and KIND[v] == 'Ghost' then return 1 end return v end
    lib_math.random = function(a, b)
        if a == nil then return rfloat() end
        a, b = one(a), one(b)
        if b == nil then a, b = 1, a end
        if type(a) ~= 'number' or type(b) ~= 'number' then error("invalid argument #1 to 'random' (number expected)", 2) end
        a, b = floor(a), floor(b)
        if a > b then error("invalid argument #2 to 'random' (interval is empty)", 2) end
        return a + floor(rfloat() * (b - a + 1))
    end
    lib_math.randomseed = function(s) rt.rand_state[1] = floor(tonumber(s) or 0) % 2147483648 end
    for name, f in next, lib_math do
        if type(f) == 'function' and name ~= 'random' and name ~= 'randomseed' then lib_math[name] = every(f) end
    end
    ENV.math = lib_math

    local real_tonumber = real.tonumber
    ENV.tonumber = function(v, base)
        if type(v) == 'userdata' and KIND[v] == 'Ghost' then return 0 end
        if base == nil then return real_tonumber(v) end
        return real_tonumber(v, base)
    end

    ENV.table = copy_table(real.table)
    first_lib(ENV.table, {'insert', 'remove', 'concat', 'sort', 'find', 'clone', 'freeze', 'isfrozen', 'getn', 'maxn', 'foreach',
        'foreachi', 'clear', 'move'}, 't')
    do
        local real_unpack = real.table.unpack
        ENV.table.unpack = function(t, i, j)
            if type(t) == 'userdata' and KIND[t] == 'Ghost' then return end
            return real_unpack(t, i, j)
        end
        ENV.unpack = ENV.table.unpack
    end
    ENV.utf8 = copy_table(real.utf8)
    ENV.bit32 = copy_table(real.bit32)
    ENV.buffer = copy_table(real.buffer)
    ENV.coroutine = copy_table(real.coroutine)

    -- time moves only when the scheduler moves it
    local real_os = real.os
    ENV.os = {
        time = function(t) if t == nil then return floor(rt.epoch()) end return real_os.time(t) end,
        clock = function() rt.query_clock(); return rt.cpu_clock() end,
        date = function(fmt, t) return real_os.date(fmt or '%c', t or floor(rt.epoch())) end,
        difftime = real_os.difftime,
    }
    -- os.execute, os.remove, os.getenv ... do not exist; a script that reaches for them is written down (once each)
    do
        local asked = {}
        setmetatable(ENV.os, {__index = function(t, k)
            if type(k) == 'string' and not asked[k] then
                asked[k] = true
                emit('probe', 'os.' .. k .. ' is not available (the script reads it and gets nil)')
            end
            return nil
        end})
    end
    ENV.tick = function() rt.query_clock(); return rt.epoch() end
    ENV.time = function() rt.query_clock(); return rt.clock() end
    ENV.elapsedTime = function() rt.query_clock(); return rt.clock() end

    local dbg = copy_table(real.debug)
    -- debug.info tells a script about its own functions and frames, never about the runtime's: those look like C functions
    do
        local real_info, CHUNKNAME = real.debug.info, rt.CHUNK
        local function c_info(opts)
            local out, n = {}, 0
            for i = 1, #opts do
                local c = ssub(opts, i, i)
                if c == 's' then n = n + 1; out[n] = '[C]'
                elseif c == 'l' then n = n + 1; out[n] = -1
                elseif c == 'n' then n = n + 1; out[n] = ''
                elseif c == 'f' then n = n + 1; out[n] = function() end
                elseif c == 'a' then out[n + 1] = 0; out[n + 2] = false; n = n + 2 end
            end
            return unpack(out, 1, n)
        end
        dbg.info = function(a, b, c)
            if type(a) == 'thread' then
                if type(b) == 'number' and type(c) == 'string' and real_info(a, b, 's') == CHUNKNAME then return c_info(c) end
                return real_info(a, b, c)
            end
            local opts = b
            if type(opts) ~= 'string' then return real_info(a, b) end
            if type(a) == 'number' then
                if real_info(a + 1, 's') == CHUNKNAME then return c_info(opts) end
                return real_info(a + 1, opts)
            end
            if type(a) == 'function' and real_info(a, 's') == CHUNKNAME then return c_info(opts) end
            return real_info(a, opts)
        end
    end
    dbg.profilebegin = function() end
    dbg.profileend = function() end
    dbg.setmemorycategory = function() end
    dbg.resetmemorycategory = function() end
    for _, n in ipairs({'getinfo', 'getupvalue', 'getupvalues', 'setupvalue', 'getconstant', 'getconstants', 'setconstant', 'getproto',
        'getprotos', 'getstack', 'setstack', 'getregistry'}) do
        dbg[n] = EX[n]
    end
    dbg.getmetatable = EX.getrawmetatable
    ENV.debug = dbg
    ENV.collectgarbage = function(opt, ...)
        if opt == nil or opt == 'collect' or opt == 'count' or opt == 'step' or opt == 'isrunning' then return real.collectgarbage(opt, ...) end
        return 0
    end
    ENV.gcinfo = function() return 600 end

    -- print and warn go to the log, not to the screen
    local function console_line(...)
        local n = select('#', ...)
        local parts = {}
        for i = 1, n do
            local ok, s = pcall(tostring, (select(i, ...)))
            parts[i] = ok and s or '?'
        end
        return tconcat(parts, ' ')
    end
    ENV.print = function(...)
        emit('out', 'print(' .. rt.args(pack(...)) .. ')', console_line(...))
    end
    ENV.warn = function(...)
        emit('out', 'warn(' .. rt.args(pack(...)) .. ')', console_line(...), 'warn')
    end
    ENV.printidentity = function() emit('out', 'printidentity()', 'Current identity is 8') end

    -- getfenv/setfenv never hand out the real globals
    local real_getfenv, real_setfenv = real.getfenv, real.setfenv
    ENV.getfenv = function(f)
        if f == nil then f = 1 end
        if type(f) == 'number' then
            if f == 0 then return ENV end
            f = f + 1
        end
        local e = real_getfenv(f)
        if e == rt.REAL_G or e == rt.RUNTIME_ENV then return ENV end
        return e
    end
    ENV.setfenv = function(f, t)
        if type(f) == 'number' then
            if f == 0 then error("invalid argument #1 to 'setfenv' (level must be positive)", 2) end
            f = f + 1
        end
        if type(t) ~= 'table' then error("invalid argument #2 to 'setfenv' (table expected)", 2) end
        return real_setfenv(f, t)
    end

    -- ------------------------------------------------------------------------------------------ the task library
    local T = {}
    T.wait = function(t) return (rt.sleep(t, 1 / 60)) end
    T.spawn = function(f, ...)
        local tf = type(f)
        if tf ~= 'function' and tf ~= 'thread' then error("invalid argument #1 to 'spawn' (function or thread expected)", 2) end
        return rt.spawn(f, ...)
    end
    T.defer = function(f, ...)
        local tf = type(f)
        if tf ~= 'function' and tf ~= 'thread' then error("invalid argument #1 to 'defer' (function or thread expected)", 2) end
        return rt.start_later(0, f, pack(...))
    end
    T.delay = function(t, f, ...)
        local tf = type(f)
        if tf ~= 'function' and tf ~= 'thread' then error("invalid argument #2 to 'delay' (function or thread expected)", 2) end
        return rt.start_later(tonumber(t) or 0, f, pack(...))
    end
    T.cancel = function(co) if type(co) == 'thread' then rt.cancel(co) end end
    T.synchronize = function() end
    T.desynchronize = function() end
    ENV.task = T
    ENV.wait = function(t)
        local el = rt.sleep(t, 0.03)
        return el, rt.clock()
    end
    ENV.Wait = ENV.wait
    ENV.spawn = function(f, ...)
        if type(f) ~= 'function' and type(f) ~= 'thread' then error("invalid argument #1 to 'spawn' (function expected)", 2) end
        return rt.start_later(0, f, pack(...))
    end
    ENV.Spawn = ENV.spawn
    ENV.delay = function(t, f, ...)
        if type(f) ~= 'function' then error("invalid argument #2 to 'delay' (function expected)", 2) end
        return rt.start_later(math.max(tonumber(t) or 0, 0.03), f, pack(...))
    end
    ENV.Delay = ENV.delay

    -- ------------------------------------------------------------------------------------------ Roblox values and objects
    ENV.game, ENV.Game = rt.game, rt.game
    local ws = rt.service('Workspace')
    ENV.workspace, ENV.Workspace = ws, ws
    local script_obj, sd = rt.newinst('LocalScript', {name = 'LocalScript', world = true, var = 'script'})
    sd.how = function() return 'script' end
    ENV.script = script_obj
    ENV.Instance = rt.Instance
    ENV.Enum = rt.Enum
    for _, n in ipairs({'Vector3', 'Vector2', 'CFrame', 'Color3', 'UDim', 'UDim2', 'BrickColor', 'Random', 'TweenInfo', 'NumberRange',
        'NumberSequence', 'NumberSequenceKeypoint', 'ColorSequence', 'ColorSequenceKeypoint', 'Rect', 'Ray', 'Region3',
        'PhysicalProperties', 'Font', 'RaycastParams', 'OverlapParams', 'CatalogSearchParams'}) do
        ENV[n] = rt[n]
    end
    ENV.require = function(m)
        local text = 'require(' .. repr(m, 1) .. ')'
        emit('call', text)
        return rt.ghost_of(text)
    end
    ENV.settings = function() return rt.ghost_of('settings()') end
    ENV.UserSettings = function() return rt.ghost_of('UserSettings()') end
    ENV.stats = function() return rt.ghost_of('stats()') end
    ENV.version = function() return '0.600.0.6000000' end
    ENV.plugin = nil

    -- DateTime
    do
        local function mk(ms) return newobj('DateTime', {ms = ms}) end
        local function parts(ms)
            local t = real_os.date('!*t', floor(ms / 1000))
            return {Year = t.year, Month = t.month, Day = t.day, Hour = t.hour, Minute = t.min, Second = t.sec, Millisecond = ms % 1000}
        end
        defkind('DateTime', {
            __index = function(self, k)
                local d = DATA[self]
                if k == 'UnixTimestamp' then return floor(d.ms / 1000)
                elseif k == 'UnixTimestampMillis' then return d.ms
                elseif k == 'ToIsoDate' then return function() return real_os.date('!%Y-%m-%dT%H:%M:%SZ', floor(d.ms / 1000)) end
                elseif k == 'ToUniversalTime' or k == 'ToLocalTime' then return function() return parts(d.ms) end
                elseif k == 'FormatUniversalTime' or k == 'FormatLocalTime' then
                    return function() return real_os.date('!%Y-%m-%d %H:%M:%S', floor(d.ms / 1000)) end
                end
                error(tostring(k) .. ' is not a valid member of DateTime', 2)
            end,
            __tostring = function() return 'DateTime' end,
            __lt = function(a, b) return DATA[a].ms < DATA[b].ms end,
            __le = function(a, b) return DATA[a].ms <= DATA[b].ms end,
        })
        rt.REPR.DateTime = function(self) return 'DateTime.fromUnixTimestampMillis(' .. DATA[self].ms .. ')' end
        ENV.DateTime = {
            now = function() return mk(floor(rt.epoch() * 1000)) end,
            fromUnixTimestamp = function(s) return mk(floor((tonumber(s) or 0) * 1000)) end,
            fromUnixTimestampMillis = function(ms) return mk(floor(tonumber(ms) or 0)) end,
            fromUniversalTime = function(y, mo, d, h, mi, s, ms)
                return mk(real_os.time({year = y or 1970, month = mo or 1, day = d or 1, hour = h or 0, min = mi or 0, sec = s or 0}) * 1000 + (ms or 0))
            end,
            fromIsoDate = function() return mk(floor(rt.epoch() * 1000)) end,
        }
        ENV.DateTime.fromLocalTime = ENV.DateTime.fromUniversalTime
    end

    -- _G and shared: writes are written down
    local function logged_table(name)
        local t = {}
        setmetatable(t, {__newindex = function(tt, k, v)
            rawset(tt, k, v)
            emit('set', name .. rt.dot(k) .. ' = ' .. repr(v, 1))
        end})
        rt.NAMED[t] = name
        return t
    end
    ENV._G = logged_table('_G')
    ENV.shared = logged_table('shared')

    -- ------------------------------------------------------------------------------------------ the executor
    for name, f in next, EX do ENV[name] = f end
    ENV.loadstring = EX.loadstring
    ENV.getgenv = function() return ENV end
    ENV.getrenv = function() return ENV end
    rt.NAMED[ENV] = 'getgenv()'
    for _, name in ipairs(CONFIG.executor_names or {}) do
        if rawget(ENV, name) == nil then
            ENV[name] = function(...)
                emit('exec', name .. '(' .. rt.args(pack(...)) .. ')')
            end
        end
    end

    -- every name a new local variable of the log must not use
    local G = {}
    for k in next, ENV do G[k] = true end
    rt.GLOBALS = G

    -- from here on, writing a global and reading one that does not exist are written down
    local probed = {}
    setmetatable(ENV, {
        __index = function(t, k)
            if type(k) == 'string' and not probed[k] then
                probed[k] = true
                emit('probe', 'global ' .. k .. ' is not defined (the script reads it and gets nil)')
            end
            return nil
        end,
        __newindex = function(t, k, v)
            rawset(t, k, v)
            if type(v) ~= 'function' then emit('set', 'getgenv()' .. rt.dot(k) .. ' = ' .. repr(v, 1)) end
        end,
    })
end
