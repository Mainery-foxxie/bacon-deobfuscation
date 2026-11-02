-- part 5: what a script executor adds to a game: request and HttpGet, files, the clipboard, hooks, loadstring, Drawing, crypt ...
-- Nothing here reaches outside: files live in a table, requests are answered by rt.fetch, hooks are written down.
do
    local emit, repr, quote, newvar = rt.emit, rt.repr, rt.quote, rt.newvar
    local newobj, defkind = rt.newobj, rt.defkind
    local fire, fire_if = rt.fire, rt.fire_if
    local EX = {}
    rt.EX = EX
    local EXECUTOR = CONFIG.executor or 'Wave'
    local EXECUTOR_VERSION = CONFIG.executor_version or '1.0.0'

    local function args_text(...) return rt.args(pack(...)) end
    -- a function the script calls: the call is written to the log
    local function ex(name, fn, kind)
        EX[name] = function(...)
            emit(kind or 'exec', name .. '(' .. rt.args(pack(...)) .. ')')
            return fn(...)
        end
    end
    -- a function that only asks: nothing is written
    local function quiet(name, fn) EX[name] = fn end

    -- ------------------------------------------------------------------------------------------ who is running
    local function ident()
        rt.probe('identifyexecutor()', EXECUTOR)
        return EXECUTOR, EXECUTOR_VERSION
    end
    quiet('identifyexecutor', ident)
    quiet('getexecutorname', function() rt.probe('getexecutorname()', EXECUTOR); return EXECUTOR end)
    quiet('getexecutorversion', function() return EXECUTOR_VERSION end)
    quiet('gethwid', function() rt.probe('gethwid()', 'E3B0C44298FC1C14'); return 'E3B0C44298FC1C149AFBF4C8996FB924' end)
    quiet('checkcaller', function() return true end)
    quiet('isrbxactive', function() return true end)
    quiet('isgameactive', function() return true end)
    quiet('isnetworkowner', function() return true end)
    local identity = 8
    quiet('getthreadidentity', function() return identity end)
    quiet('getidentity', function() return identity end)
    quiet('get_thread_identity', function() return identity end)
    quiet('setthreadidentity', function(n) identity = n end)
    quiet('setidentity', function(n) identity = n end)
    quiet('set_thread_identity', function(n) identity = n end)
    quiet('getfpscap', function() return 60 end)
    ex('setfpscap', function() end)

    -- ------------------------------------------------------------------------------------------ functions
    local CCL = rt.weak('k')                  -- functions made with newcclosure
    local function is_c(f)
        if CCL[f] then return true end
        local ok, src = pcall(debug.info, f, 's')
        return ok and src == '[C]'
    end
    quiet('iscclosure', function(f) return type(f) == 'function' and is_c(f) end)
    quiet('islclosure', function(f) return type(f) == 'function' and not is_c(f) end)
    quiet('isluaclosure', function(f) return type(f) == 'function' and not is_c(f) end)
    quiet('isexecutorclosure', function(f) return type(f) == 'function' and not rt.NAMED[f] and (CCL[f] or not is_c(f)) or false end)
    quiet('isourclosure', EX.isexecutorclosure)
    quiet('is_synapse_function', EX.isexecutorclosure)
    quiet('newcclosure', function(f)
        if type(f) ~= 'function' then error('invalid argument #1 to \'newcclosure\' (function expected)', 2) end
        local w = function(...) return f(...) end
        CCL[w] = true
        return w
    end)
    quiet('newlclosure', function(f) return function(...) return f(...) end end)
    quiet('clonefunction', function(f)
        if type(f) ~= 'function' then error('invalid argument #1 to \'clonefunction\' (function expected)', 2) end
        local w = function(...) return f(...) end
        if CCL[f] or is_c(f) then CCL[w] = true end
        return w
    end)
    quiet('cloneref', function(inst) return inst end)
    quiet('compareinstances', function(a, b) return rawequal(a, b) end)
    quiet('getfunctionhash', function(f) return '0000000000000000000000000000000000000000000000000000000000000000' end)
    quiet('comparefunctions', function(a, b) return rawequal(a, b) end)
    quiet('comparefunction', function(a, b) return rawequal(a, b) end)
    quiet('isfunctionhooked', function() return false end)
    quiet('restorefunction', function() end)
    quiet('getconstants', function() return {} end)
    quiet('getconstant', function() return nil end)
    quiet('setconstant', function() end)
    quiet('getupvalues', function() return {} end)
    quiet('getupvalue', function() return nil end)
    quiet('setupvalue', function() end)
    quiet('getprotos', function() return {} end)
    quiet('getproto', function() return nil end)
    quiet('getstack', function() return {} end)
    quiet('setstack', function() end)
    quiet('getregistry', function() return {} end)
    quiet('getreg', function() return {} end)
    quiet('getgc', function() return {} end)
    quiet('get_gc_objects', function() return {} end)
    quiet('getsenv', function() return {} end)
    quiet('getmenv', function() return {} end)
    quiet('getrunningscripts', function() return {} end)
    quiet('getscripts', function() return {} end)
    quiet('getloadedmodules', function() return {} end)
    quiet('getnilinstances', function() return {} end)
    quiet('getcallingscript', function() return nil end)
    quiet('getscriptbytecode', function() return '' end)
    quiet('getscripthash', function() return '0' end)
    quiet('getscriptclosure', function() return function() return {} end end)
    quiet('decompile', function() return '-- decompiled by the sandbox: nothing to show\n' end)
    quiet('getthreadcontext', function() return identity end)
    quiet('getcustomasset', function(path)
        emit('file', 'getcustomasset(' .. repr(path, 1) .. ')')
        return 'rbxasset://' .. tostring(path)
    end)
    quiet('getinfo', function(f)
        local src, line, name, nparams, vararg, fn
        if type(f) == 'number' then
            src, line, name, fn = debug.info(f + 1, 'slnf')
        else
            src, line, name = debug.info(f, 'sln')
            nparams, vararg = debug.info(f, 'a')
            fn = f
        end
        if src == rt.CHUNK then src, line, name, fn = '[C]', -1, nil, nil end   -- the runtime's own functions are not shown
        local c = (src == '[C]')
        return {source = c and '=[C]' or ('@' .. tostring(src)), short_src = tostring(src), currentline = line or -1, linedefined = line or -1,
            lastlinedefined = line or -1, what = c and 'C' or 'Lua', name = name or '', func = fn, numparams = nparams or 0,
            is_vararg = vararg and 1 or 0, nups = 0, namewhat = ''}
    end)

    -- hooks are written down, not installed; the function the script gets back calls the original
    ex('hookfunction', function(f, hook)
        if type(f) ~= 'function' then error('invalid argument #1 to \'hookfunction\' (function expected)', 2) end
        local w = function(...) return f(...) end
        if CCL[f] or is_c(f) then CCL[w] = true end
        return w
    end)
    EX.replaceclosure = EX.hookfunction
    local namecall_name = ''
    quiet('getnamecallmethod', function() return namecall_name end)
    quiet('setnamecallmethod', function(n) namecall_name = tostring(n) end)
    quiet('get_namecall_method', EX.getnamecallmethod)
    quiet('set_namecall_method', EX.setnamecallmethod)
    ex('hookmetamethod', function(obj, name, hook)
        local mt = EX.getrawmetatable(obj)
        local orig = mt[name]
        local w = function(...)
            if orig ~= nil then return orig(...) end
        end
        CCL[w] = true
        return w
    end)

    -- a metatable that scripts can read and write; writes are written down
    local fakemt = {}
    local function meta_for(obj)
        local k = KIND[obj]
        if k == nil then
            local t = type(obj)
            if t == 'table' or t == 'string' then return getmetatable(obj) end
            return nil
        end
        local m = fakemt[k]
        if m then return m end
        local store = {}
        local function cfn(f) CCL[f] = true; return f end
        store.__index = cfn(function(o, key) return o[key] end)
        store.__newindex = cfn(function(o, key, v) o[key] = v end)
        store.__namecall = cfn(function(o, ...)
            local f = o[namecall_name]
            if f ~= nil then return f(o, ...) end
        end)
        store.__tostring = cfn(function(o) return tostring(o) end)
        store.__eq = cfn(function(a, b) return rawequal(a, b) end)
        m = setmetatable({}, {
            __index = store,
            __newindex = function(t, key, v)
                store[key] = v
                emit('exec', 'getrawmetatable(' .. k .. ').' .. tostring(key) .. ' = ' .. repr(v, 1))
            end,
            __metatable = 'locked',
        })
        fakemt[k] = m
        return m
    end
    quiet('getrawmetatable', meta_for)
    ex('setrawmetatable', function(obj, mt)
        if type(obj) == 'table' then setmetatable(obj, mt) end
        return obj
    end)
    quiet('getmetafield', function(obj, key) local m = meta_for(obj); return m and m[key] or nil end)
    quiet('hasmetafield', function(obj, key) local m = meta_for(obj); return m ~= nil and m[key] ~= nil end)
    quiet('hasmetatable', function(obj) return meta_for(obj) ~= nil end)
    quiet('setreadonly', function(t, b)
        if type(t) == 'table' and b == true then pcall(table.freeze, t) end
    end)
    quiet('isreadonly', function(t) return type(t) == 'table' and table.isfrozen(t) end)
    quiet('make_writeable', function() end)
    quiet('makewriteable', function() end)
    quiet('make_readonly', function() end)
    quiet('makereadonly', function() end)

    -- ------------------------------------------------------------------------------------------ the instance tree
    local function descend(inst, out)
        local kids = DATA[inst].kids
        for i = 1, #kids do
            out[#out + 1] = kids[i]
            descend(kids[i], out)
        end
        return out
    end
    quiet('getinstances', function() return descend(rt.game, {}) end)
    quiet('getinstancesbyclass', function() return {} end)
    quiet('getinstancesofclass', function() return {} end)
    quiet('gethui', function()
        local h = rt.hidden_ui
        if h == nil then
            local d
            h, d = rt.newinst('Folder', {name = 'HiddenUI', world = true})
            d.strict = true   -- it holds only what the script itself puts there
            d.how = function() return 'gethui()' end
            d.varname = 'HiddenUI'
            rt.hidden_ui = h
        end
        return h
    end)
    quiet('gethiddenproperty', function(inst, prop)
        if KIND[inst] ~= 'Instance' then return nil, false end
        return inst[prop], false
    end)
    ex('sethiddenproperty', function(inst, prop, v)
        if KIND[inst] == 'Instance' then rt.setprop(inst, prop, v) end
        return true
    end)
    ex('setscriptable', function() return true end)
    quiet('isscriptable', function() return true end)
    quiet('getcallbackvalue', function(inst, prop)
        if KIND[inst] == 'Instance' then return DATA[inst].props[prop] end
        return nil
    end)
    ex('saveinstance', function() end)
    ex('firesignal', function(sig, ...)
        if KIND[sig] == 'RBXScriptSignal' then fire(sig, ...) end
    end)
    EX.replicatesignal = EX.firesignal
    quiet('getconnections', function(sig)
        local out = {}
        if KIND[sig] == 'RBXScriptSignal' then
            local conns = DATA[sig].conns
            for i = 1, #conns do
                local c = conns[i]
                local cd = DATA[c]
                out[#out + 1] = {
                    Function = cd.fn, Enabled = cd.on, State = cd.on, ForeignState = false, LuaConnection = true,
                    Fire = function(self, ...) rt.spawn(cd.fn, ...) end,
                    Defer = function(self, ...) rt.start_later(0, cd.fn, pack(...)) end,
                    Disable = function() cd.on = false end,
                    Enable = function() cd.on = true end,
                    Disconnect = function() rt.disconnect(c) end,
                }
            end
        end
        return out
    end)
    local function local_player() return rt.players[1] end
    ex('fireclickdetector', function(cd, dist)
        if KIND[cd] == 'Instance' then
            fire_if(cd, 'MouseClick', local_player())
            fire_if(cd, 'MouseHoverEnter', local_player())
        end
    end)
    ex('fireproximityprompt', function(pp)
        if KIND[pp] == 'Instance' then
            fire_if(pp, 'PromptButtonHoldBegan', local_player())
            fire_if(pp, 'Triggered', local_player())
            fire_if(pp, 'PromptButtonHoldEnded', local_player())
            fire_if(pp, 'TriggerEnded', local_player())
        end
    end)
    ex('firetouchinterest', function(a, b, toggle)
        if KIND[a] ~= 'Instance' or KIND[b] ~= 'Instance' then return end
        local begin = (toggle == 0 or toggle == true)
        fire_if(a, begin and 'Touched' or 'TouchEnded', b)
        fire_if(b, begin and 'Touched' or 'TouchEnded', a)
    end)

    -- ------------------------------------------------------------------------------------------ input, clipboard, windows
    for _, n in ipairs({'mouse1click', 'mouse1press', 'mouse1release', 'mouse2click', 'mouse2press', 'mouse2release', 'mousemoverel',
        'mousemoveabs', 'mousescroll', 'keypress', 'keyrelease', 'keyclick', 'rconsoleclear', 'rconsolecreate', 'rconsoledestroy',
        'rconsolehide', 'rconsoleshow', 'rconsolename', 'consolecreate', 'consoleclear', 'consoledestroy', 'consolesettitle'}) do
        ex(n, function() end)
    end
    for _, n in ipairs({'rconsoleprint', 'rconsoleinfo', 'rconsolewarn', 'rconsoleerr', 'consoleprint'}) do
        EX[n] = function(text)
            emit('out', n .. '(' .. repr(text, 1) .. ')', tostring(text))
        end
    end
    quiet('rconsoleinput', function() return '' end)
    quiet('consoleinput', function() return '' end)
    local clipboard = ''
    local function set_clip(text)
        emit('exec', 'setclipboard(' .. quote(tostring(text), 200) .. ')')
        clipboard = tostring(text)
        if #clipboard > 200 then rt.payload('clipboard', 'setclipboard', clipboard) end
    end
    EX.setclipboard = set_clip
    EX.toclipboard = set_clip
    EX.setrbxclipboard = set_clip
    quiet('getclipboard', function() return clipboard end)
    ex('messagebox', function() return 1 end)
    quiet('openfiledialog', function() return nil end)
    quiet('openfilesdialog', function() return {} end)
    quiet('openfolderdialog', function() return nil end)
    quiet('savefiledialog', function() return nil end)

    -- ------------------------------------------------------------------------------------------ teleports
    EX.queue_on_teleport = function(code)
        local s = tostring(code)
        emit('exec', 'queue_on_teleport(' .. quote(s, 120) .. ')')
        rt.payload('queue_on_teleport', 'queue_on_teleport', s)
    end
    EX.queueonteleport = EX.queue_on_teleport
    ex('clear_teleport_queue', function() end)

    -- ------------------------------------------------------------------------------------------ files
    local FS, DIRS = {}, {}
    for path, text in next, (CONFIG.files or {}) do FS[path] = text end
    local function norm(p)
        p = tostring(p)
        p = sgsub(p, '\\', '/')
        p = sgsub(p, '/+$', '')
        return p
    end
    local function must_path(name, p)
        if type(p) ~= 'string' then error('invalid argument #1 to \'' .. name .. '\' (string expected, got ' .. type(p) .. ')', 3) end
        return norm(p)
    end
    local function parent_of(p) return string.match(p, '^(.*)/[^/]*$') end
    EX.writefile = function(path, data)
        local p = must_path('writefile', path)
        local s = type(data) == 'string' and data or tostring(data)
        emit('file', 'writefile(' .. quote(p, 160) .. ', ' .. #s .. ' bytes)', p)
        FS[p] = s
        rt.payload('file', p, s)
    end
    EX.appendfile = function(path, data)
        local p = must_path('appendfile', path)
        local s = type(data) == 'string' and data or tostring(data)
        emit('file', 'appendfile(' .. quote(p, 160) .. ', ' .. #s .. ' bytes)', p)
        FS[p] = (FS[p] or '') .. s
    end
    EX.readfile = function(path)
        local p = must_path('readfile', path)
        local s = FS[p]
        emit('file', 'readfile(' .. quote(p, 160) .. ') --> ' .. (s and (#s .. ' bytes') or 'does not exist'), p)
        if s == nil then error(p .. ' does not exist', 2) end
        return s
    end
    EX.isfile = function(path)
        local p = must_path('isfile', path)
        local r = FS[p] ~= nil
        emit('file', 'isfile(' .. quote(p, 160) .. ') --> ' .. tostring(r), p)
        return r
    end
    EX.isfolder = function(path)
        local p = must_path('isfolder', path)
        local r = DIRS[p] == true
        if not r then
            local prefix = p .. '/'
            for f in next, FS do
                if ssub(f, 1, #prefix) == prefix then r = true; break end
            end
        end
        emit('file', 'isfolder(' .. quote(p, 160) .. ') --> ' .. tostring(r), p)
        return r
    end
    EX.makefolder = function(path)
        local p = must_path('makefolder', path)
        emit('file', 'makefolder(' .. quote(p, 160) .. ')', p)
        DIRS[p] = true
    end
    EX.delfile = function(path)
        local p = must_path('delfile', path)
        emit('file', 'delfile(' .. quote(p, 160) .. ')', p)
        FS[p] = nil
    end
    EX.delfolder = function(path)
        local p = must_path('delfolder', path)
        emit('file', 'delfolder(' .. quote(p, 160) .. ')', p)
        DIRS[p] = nil
        local prefix = p .. '/'
        for f in next, FS do
            if ssub(f, 1, #prefix) == prefix then FS[f] = nil end
        end
    end
    EX.listfiles = function(path)
        local p = must_path('listfiles', path)
        emit('file', 'listfiles(' .. quote(p, 160) .. ')', p)
        local prefix = p == '' and '' or (p .. '/')
        local seen, out = {}, {}
        for f in next, FS do
            if ssub(f, 1, #prefix) == prefix then
                local rest = ssub(f, #prefix + 1)
                local first = string.match(rest, '^[^/]*')
                local entry = prefix .. first
                if not seen[entry] then seen[entry] = true; out[#out + 1] = entry end
            end
        end
        table.sort(out)
        return out
    end
    EX.loadfile = function(path)
        local p = must_path('loadfile', path)
        local s = FS[p]
        emit('file', 'loadfile(' .. quote(p, 160) .. ') --> ' .. (s and (#s .. ' bytes') or 'does not exist'), p)
        if s == nil then return nil, p .. ' does not exist' end
        return EX.loadstring(s, p)
    end
    EX.dofile = function(path)
        local p = must_path('dofile', path)
        local s = FS[p]
        emit('file', 'dofile(' .. quote(p, 160) .. ') --> ' .. (s and (#s .. ' bytes') or 'does not exist'), p)
        if s == nil then error(p .. ' does not exist', 2) end
        local f, err = EX.loadstring(s, p)
        if f then return f() end
        error(err, 2)
    end

    -- ------------------------------------------------------------------------------------------ requests
    local function mkrequest(name)
        return function(opts)
            if type(opts) ~= 'table' then error('invalid argument #1 to \'' .. name .. '\' (table expected, got ' .. type(opts) .. ')', 2) end
            local url = opts.Url or opts.url
            if type(url) ~= 'string' then error('Invalid or missing Url', 2) end
            local method = string.upper(tostring(opts.Method or opts.method or 'GET'))
            local body = opts.Body or opts.body
            return rt.http_response(name .. '(' .. repr(opts, 0) .. ')', url, method, type(body) == 'string' and body or nil)
        end
    end
    EX.request = mkrequest('request')
    EX.http_request = mkrequest('http_request')
    EX.http = {request = mkrequest('http.request'), get = function(url) return rt.fetch('http.get(' .. quote(tostring(url), 300) .. ')', tostring(url), 'GET') end}
    EX.syn = nil

    -- loadstring: the text is kept, then run inside the pretend world
    local nloads = 0
    local LOADMAX = CONFIG.max_loads or 400
    local real_loadstring = loadstring
    EX.loadstring = function(src, chunkname)
        if type(src) ~= 'string' then
            error('invalid argument #1 to \'loadstring\' (string expected, got ' .. type(src) .. ')', 2)
        end
        nloads = nloads + 1
        local label = type(chunkname) == 'string' and chunkname or 'loadstring'
        local id = #src > 0 and rt.payload('loadstring', label, src) or nil
        emit('load', 'loadstring(' .. quote(src, 100) .. ')', id, #src)
        if nloads > LOADMAX then return nil, 'sandbox limit: too many loadstring calls' end
        local fn, err = real_loadstring(src, '=' .. label)
        if not fn then return nil, err end
        setfenv(fn, rt.ENV)
        return fn
    end

    -- ------------------------------------------------------------------------------------------ crypt
    local B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
    local B64I = {}
    for i = 1, 64 do B64I[sbyte(B64, i)] = i - 1 end
    local function b64enc(s)
        local out = {}
        for i = 1, #s, 3 do
            local a, b, c = sbyte(s, i, i + 2)
            local n = a * 65536 + (b or 0) * 256 + (c or 0)
            local c1, c2, c3, c4 = floor(n / 262144) % 64, floor(n / 4096) % 64, floor(n / 64) % 64, n % 64
            out[#out + 1] = ssub(B64, c1 + 1, c1 + 1) .. ssub(B64, c2 + 1, c2 + 1)
                .. (b and ssub(B64, c3 + 1, c3 + 1) or '=') .. (c and ssub(B64, c4 + 1, c4 + 1) or '=')
        end
        return tconcat(out)
    end
    local function b64dec(s)
        s = sgsub(tostring(s), '[^%w%+/]', '')
        local out = {}
        for i = 1, #s, 4 do
            local a, b, c, d = sbyte(s, i, i + 3)
            local v = (B64I[a] or 0) * 262144 + (B64I[b] or 0) * 4096 + (c and B64I[c] or 0) * 64 + (d and B64I[d] or 0)
            local chunk = string.char(floor(v / 65536) % 256, floor(v / 256) % 256, v % 256)
            if not c then chunk = ssub(chunk, 1, 1) elseif not d then chunk = ssub(chunk, 1, 2) end
            out[#out + 1] = chunk
        end
        return tconcat(out)
    end
    local function xor_text(data, key)
        local out, kl = {}, #key
        if kl == 0 then return data end
        for i = 1, #data do out[i] = string.char(bit32.bxor(sbyte(data, i), sbyte(key, (i - 1) % kl + 1))) end
        return tconcat(out)
    end
    local function rand_bytes(n)
        local t = {}
        for i = 1, n do t[i] = string.char(rt.rand_int(256) - 1) end
        return tconcat(t)
    end
    local HLEN = {md5 = 32, sha1 = 40, sha224 = 56, sha256 = 64, sha384 = 96, sha512 = 128, md2 = 32, md4 = 32}
    -- not a real hash: the same text always gives the same digits, of the length that name has
    local function fake_hash(data, alg)
        alg = string.lower(tostring(alg or 'sha256'))
        alg = sgsub(alg, '[^%w]', '')
        local want = HLEN[alg] or 64
        local h1, h2 = 5381, 52711
        for i = 1, #data do
            local c = sbyte(data, i)
            h1 = (h1 * 33 + c) % 4294967296
            h2 = (h2 * 31 + c * 7 + i) % 4294967296
        end
        local out, a, b = {}, h1, h2
        while #out * 8 < want do
            a = (a * 1664525 + 1013904223) % 4294967296
            b = (b * 22695477 + 1) % 4294967296
            out[#out + 1] = sfmt('%08x', (a + b) % 4294967296)
        end
        return ssub(tconcat(out), 1, want)
    end
    local crypt = {
        base64encode = b64enc, base64_encode = b64enc, base64decode = b64dec, base64_decode = b64dec,
        encrypt = function(data, key, iv) return b64enc(xor_text(tostring(data), tostring(key))), iv or b64enc(rand_bytes(16)) end,
        decrypt = function(data, key) return xor_text(b64dec(data), tostring(key)) end,
        generatebytes = function(n) return b64enc(rand_bytes(type(n) == 'number' and n or 16)) end,
        generatekey = function() return b64enc(rand_bytes(32)) end,
        hash = function(data, alg) return fake_hash(tostring(data), alg) end,
    }
    crypt.base64 = {encode = b64enc, decode = b64dec}
    crypt.custom = {hash = function(alg, data) return fake_hash(tostring(data), alg) end, encrypt = crypt.encrypt, decrypt = crypt.decrypt}
    EX.crypt = crypt
    EX.base64_encode = b64enc
    EX.base64_decode = b64dec
    EX.base64encode = b64enc
    EX.base64decode = b64dec
    EX.lz4compress = function(s) return s end
    EX.lz4decompress = function(s) return s end
    EX.json = {encode = function(v) return rt.json_encode(v) end, decode = function(s) return rt.json_decode(s) end}
    EX.cache = {invalidate = function() end, iscached = function() return true end, replace = function() end}

    -- ------------------------------------------------------------------------------------------ WebSocket
    defkind('WebSocket', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'Send' then
                return function(me, msg)
                    emit('net', d.var .. ':Send(' .. repr(msg, 1) .. ')', d.url, 'WS SEND')
                    if type(msg) == 'string' and #msg > 0 then rt.payload('body', 'WS SEND ' .. d.url, msg) end
                end
            elseif k == 'Close' then
                return function(me) emit('net', d.var .. ':Close()', d.url, 'WS CLOSE') end
            elseif k == 'OnMessage' then return d.onmessage
            elseif k == 'OnClose' then return d.onclose
            end
            error(tostring(k) .. ' is not a valid member of WebSocket', 2)
        end,
        __tostring = function() return 'WebSocket' end,
    })
    rt.REPR.WebSocket = function(self) return DATA[self].var end
    EX.WebSocket = {connect = function(url)
        if type(url) ~= 'string' then error('invalid argument #1 to \'connect\' (string expected)', 2) end
        local var = newvar('WebSocket')
        local d = {url = url, var = var}
        d.onmessage = rt.newsig(nil, 'OnMessage', function() return var .. '.OnMessage' end)
        d.onclose = rt.newsig(nil, 'OnClose', function() return var .. '.OnClose' end)
        emit('net', 'local ' .. var .. ' = WebSocket.connect(' .. quote(url, 300) .. ')', url, 'WS CONNECT')
        return newobj('WebSocket', d)
    end}

    -- ------------------------------------------------------------------------------------------ Drawing
    local DRAW_DEFAULT = {
        Visible = false, Transparency = 1, Color = rt.C3(0, 0, 0), ZIndex = 0, Thickness = 1, Filled = false, NumSides = 0, Radius = 0,
        Text = '', Size = rt.V2(0, 0), Position = rt.V2(0, 0), From = rt.V2(0, 0), To = rt.V2(0, 0), Center = false, Outline = false,
        OutlineColor = rt.C3(0, 0, 0), Font = 0, TextBounds = rt.V2(0, 0), PointA = rt.V2(0, 0), PointB = rt.V2(0, 0),
        PointC = rt.V2(0, 0), PointD = rt.V2(0, 0), Data = '', Rounding = 0,
    }
    local DRAW_TYPES = {Line = true, Text = true, Image = true, Circle = true, Square = true, Quad = true, Triangle = true}
    defkind('Drawing', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'Remove' or k == 'Destroy' then
                return function(me)
                    emit('call', d.var .. ':' .. k .. '()')
                    d.removed = true
                end
            end
            local v = d.props[k]
            if v ~= nil then return v end
            if k == 'ClassName' then return d.cls end
            return DRAW_DEFAULT[k]
        end,
        __newindex = function(self, k, v)
            local d = DATA[self]
            d.props[k] = v
            emit('set', d.var .. rt.dot(k) .. ' = ' .. repr(v, 1))
        end,
        __tostring = function() return 'Drawing' end,
    })
    rt.REPR.Drawing = function(self) return DATA[self].var end
    EX.Drawing = {
        new = function(kind)
            if not DRAW_TYPES[kind] then error('Invalid drawing type', 2) end
            local var = newvar(kind)
            emit('def', 'local ' .. var .. ' = Drawing.new(' .. quote(kind) .. ')', kind)
            return newobj('Drawing', {cls = kind, props = {}, var = var})
        end,
        Fonts = {UI = 0, System = 1, Plex = 2, Monospace = 3},
    }
    quiet('cleardrawcache', function() end)
    quiet('isrenderobj', function(o) return KIND[o] == 'Drawing' end)
    quiet('getrenderproperty', function(o, k) return KIND[o] == 'Drawing' and o[k] or nil end)
    ex('setrenderproperty', function(o, k, v) if KIND[o] == 'Drawing' then DATA[o].props[k] = v end end)
end
