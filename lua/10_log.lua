-- part 1: the event log.  Every line the runtime prints starts with a control character, so a script can not forge one:
--   \1T <tab> time[@line] <tab> kind <tab> text [<tab> a [<tab> b]]    an event (text and fields are escaped: \\ \n \r \t \xNN)
--   \1P <tab> id <tab> kind <tab> label <tab> length <tab> text   a captured payload (source of loadstring, a written file...)
--   \1S <tab> times <tab> kind <tab> text <tab> first <tab> last   events that were left out because they kept repeating
--   \1X <tab> reason <tab> key=value ...                          the end
do
    local MAXEV = CONFIG.max_events or 8000
    local PAD = CONFIG.pad or 0
    local padline = PAD > 0 and srep(' ', PAD) or nil
    local shown, total, truncated = 0, 0, false
    local seen, suppressed, supn = {}, {}, 0
    local npay, paybytes = 0, 0

    -- limits per kind: how many events of one shape are written (numbers in the text do not count as a difference)
    local LIMITS = {set = 4, call = 4, remote = 12, exec = 6, conn = 8, out = 40, file = 12, misc = 6}
    local NOSUP = {def = true, net = true, load = true, err = true, probe = true, note = true, kick = true, tp = true}
    rt.LIMITS = LIMITS

    local function esc(s)
        return (sgsub(s, '[%c\\\127]', function(c)
            if c == '\\' then return '\\\\' elseif c == '\n' then return '\\n' elseif c == '\r' then return '\\r'
            elseif c == '\t' then return '\\t' end
            return sfmt('\\x%02X', sbyte(c))
        end))
    end
    rt.esc = esc

    local function line(s)
        print(s)
        if padline then print(padline) end
    end

    -- the text without its digits: `Frame37.Size = UDim2.fromOffset(10, 20)` and `Frame38.Size = UDim2.fromOffset(30, 40)` look alike
    local function shape(text)
        return (sgsub(text, '%d+', '#'))
    end

    local function cut(s, n)
        if #s > n then return ssub(s, 1, n) .. sfmt(' --[[+%d]]', #s - n) end
        return s
    end

    -- where the script is now: the line of the first frame that does not belong to the runtime ("12" in the main script,
    -- "loadstring:3" in a payload), or nil
    local MAIN = CONFIG.chunkname or 'script'
    rt.MAIN = MAIN
    function rt.where()
        local info, own = debug.info, rt.CHUNK
        for lvl = 1, 80 do
            local src, ln = info(lvl, 'sl')
            if src == nil then return nil end
            if src ~= own and src ~= '[C]' and ln and ln > 0 then
                if src == MAIN then return tostring(ln) end
                return src .. ':' .. ln
            end
        end
        return nil
    end

    -- one event.  kind: def set call conn net remote load exec out err probe file kick tp note misc
    function rt.emit(kind, text, a, b, at)
        total = total + 1
        if not NOSUP[kind] then
            local key = kind .. '\0' .. shape(text)
            local n = (seen[key] or 0) + 1
            seen[key] = n
            if n > (LIMITS[kind] or 6) then
                local s = suppressed[key]
                if not s then
                    supn = supn + 1
                    s = {n = 0, kind = kind, text = text, first = rt.clock()}
                    suppressed[key] = s
                end
                s.n = s.n + 1
                s.last = rt.clock()
                return false
            end
        end
        if shown >= MAXEV then
            truncated = true
            return false
        end
        shown = shown + 1
        at = at or rt.where()
        local f = '\1T\t' .. sfmt('%.3f', rt.clock()) .. (at and ('@' .. at) or '') .. '\t' .. kind .. '\t' .. esc(cut(text, 1500))
        if a ~= nil then f = f .. '\t' .. esc(cut(tostring(a), 1500)) end
        if b ~= nil then f = f .. '\t' .. esc(cut(tostring(b), 1500)) end
        line(f)
        return true
    end

    -- a captured text (the source passed to loadstring, a file the script wrote, ...) -> id.  The same text again (a script
    -- that calls queue_on_teleport every few seconds) is the same payload.
    local bydata = {}
    function rt.payload(kind, label, data)
        local dupkey = kind .. '\0' .. data
        if bydata[dupkey] then return bydata[dupkey] end
        local cap = CONFIG.max_payload or 2000000
        local room = (CONFIG.max_payloads_total or 8000000) - paybytes
        npay = npay + 1
        local id = npay
        bydata[dupkey] = id
        local keep = data
        if #keep > cap then keep = ssub(keep, 1, cap) end
        if #keep > room then keep = ssub(keep, 1, room > 0 and room or 0) end
        paybytes = paybytes + #keep
        line('\1P\t' .. id .. '\t' .. kind .. '\t' .. esc(cut(label, 300)) .. '\t' .. #data .. '\t' .. esc(keep))
        return id
    end

    function rt.finish_log(reason, extra)
        local list = {}
        for _, s in pairs(suppressed) do list[#list + 1] = s end
        table.sort(list, function(x, y)
            if x.n ~= y.n then return x.n > y.n end
            return x.text < y.text
        end)
        for i = 1, math.min(#list, 60) do
            local s = list[i]
            line('\1S\t' .. s.n .. '\t' .. s.kind .. '\t' .. esc(cut(s.text, 600)) .. '\t' .. sfmt('%.3f', s.first) .. '\t' .. sfmt('%.3f', s.last))
        end
        local f = '\1X\t' .. esc(reason) .. '\tevents=' .. total .. '\tshown=' .. shown .. '\tcut=' .. (truncated and 1 or 0)
            .. '\tsuppressed=' .. supn .. '\tpayloads=' .. npay
        if extra then f = f .. '\t' .. esc(extra) end
        line(f)
    end
end

-- ------------------------------------------------------------------------------------------------ showing values
do
    local RESERVED = {}
    for w in string.gmatch('and break continue do else elseif end false for function if in local nil not or repeat return '
        .. 'then true until while export type typeof', '%a+') do RESERVED[w] = true end
    local varcount = {}

    -- a Lua name from any text: `Main Frame` -> MainFrame, `3D` -> _3D
    local function ident(s)
        s = sgsub(tostring(s), '[^%w_]', '')
        if s == '' then s = 'obj' end
        if sbyte(s) < 58 and sbyte(s) > 47 then s = '_' .. s end
        if #s > 40 then s = ssub(s, 1, 40) end
        return s
    end
    rt.ident = ident

    -- a name nothing else in the log has used, and that is not a global of the script
    function rt.newvar(base)
        base = ident(base)
        if RESERVED[base] or (rt.GLOBALS and rt.GLOBALS[base]) then base = base .. '_' end
        local n = (varcount[base] or 0) + 1
        varcount[base] = n
        if n == 1 then return base end
        return base .. n
    end
    rt.RESERVED = RESERVED

    local ESC = {['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t', ['"'] = '\\"', ['\\'] = '\\\\', ['\0'] = '\\0'}
    local function quote(s, max)
        max = max or 240
        local extra = ''
        if #s > max then
            extra = sfmt(' --[[+%d bytes]]', #s - max)
            s = ssub(s, 1, max)
        end
        s = sgsub(s, '[%c"\\\127]', function(c) return ESC[c] or sfmt('\\%03d', sbyte(c)) end)
        return '"' .. s .. '"' .. extra
    end
    rt.quote = quote

    local function num(n)
        if n ~= n then return '0/0' end
        if n == huge then return 'math.huge' elseif n == -huge then return '-math.huge' end
        if n == floor(n) and n > -1e15 and n < 1e15 then return sfmt('%d', n) end
        return tostring(n)
    end
    rt.num = num

    -- how a table key is written
    local function key(k)
        if type(k) == 'string' then
            if string.find(k, '^[%a_][%w_]*$') and not RESERVED[k] then return k end
            return '[' .. quote(k, 60) .. ']'
        end
        return '[' .. rt.repr(k, 3) .. ']'
    end

    rt.REPR = {}                -- kind of a fake object -> function(object, depth) -> text
    rt.NAMED = weak()           -- table/function -> a name to show instead of its contents (getgenv(), _G, shared ...)

    local function repr(v, depth, seen)
        depth = depth or 0
        local t = type(v)
        if t == 'nil' then return 'nil'
        elseif t == 'boolean' then return v and 'true' or 'false'
        elseif t == 'number' then return num(v)
        elseif t == 'string' then return quote(v)
        elseif t == 'function' then
            local nm = rt.NAMED[v]
            if nm then return nm end
            local ok, src, ln = pcall(debug.info, v, 'sl')
            if ok and (src == '[C]' or src == rt.CHUNK) then return 'function() --[[C]] end' end   -- the runtime's own are builtins to the script
            if ok and ln then return 'function() --[[line ' .. ln .. ']] end' end
            return 'function() end'
        elseif t == 'table' then
            local nm = rt.NAMED[v]
            if nm then return nm end
            seen = seen or {}
            if seen[v] or depth >= 3 then return '{...}' end
            seen[v] = true
            local parts, n = {}, rawlen(v)
            for i = 1, math.min(n, 8) do parts[#parts + 1] = repr(rawget(v, i), depth + 1, seen) end
            if n > 8 then parts[#parts + 1] = sfmt('--[[+%d]]', n - 8) end
            local keys = {}
            for k in next, v do
                if not (type(k) == 'number' and k >= 1 and k <= n and k == floor(k)) then keys[#keys + 1] = k end
            end
            if #keys > 1 then
                -- the same order every time: strings by name, then numbers, then the rest
                table.sort(keys, function(a, b)
                    local ta, tb = type(a), type(b)
                    if ta ~= tb then return ta < tb end
                    if ta == 'string' or ta == 'number' then return a < b end
                    return false
                end)
            end
            for i = 1, #keys do
                if i > 8 then parts[#parts + 1] = '...'; break end
                parts[#parts + 1] = key(keys[i]) .. ' = ' .. repr(rawget(v, keys[i]), depth + 1, seen)
            end
            seen[v] = nil
            return '{' .. tconcat(parts, ', ') .. '}'
        elseif t == 'userdata' then
            local k = KIND[v]
            local f = k and rt.REPR[k]
            if f then return f(v, depth) end
            return 'userdata'
        elseif t == 'thread' then return 'coroutine.create(function() end)'
        elseif t == 'buffer' then return 'buffer.create(' .. buffer.len(v) .. ')'
        elseif t == 'vector' then return 'vector.create(' .. num(v.x) .. ', ' .. num(v.y) .. ', ' .. num(v.z) .. ')'
        end
        return '<' .. t .. '>'
    end
    rt.repr = repr

    -- the arguments of a call: `1, "a", Vector3.new(0, 1, 0)`, at most ~400 characters
    function rt.args(t, n)
        local parts, len = {}, 0
        for i = 1, n or t.n or #t do
            local s = repr(t[i], 1)
            len = len + #s + 2
            if len > 500 then parts[#parts + 1] = '...'; break end
            parts[#parts + 1] = s
        end
        return tconcat(parts, ', ')
    end
end
