-- part 4c: the services that scripts use all the time: frames, input, tweens, HTTP and JSON, teleports, notifications ...
do
    local newinst, defm, ref, attach, mark_child = rt.newinst, rt.defm, rt.ref, rt.attach, rt.mark_child
    local DEF = rt.DEF
    local V3, V2, CF = rt.V3, rt.V2, rt.CF
    local E = rt.enum_item
    local emit, repr, quote = rt.emit, rt.repr, rt.quote
    local fire, fire_if, setprop = rt.fire, rt.fire_if, rt.setprop
    local SI = rt.SERVICE_INIT

    local function copy(t)
        local o = {}
        for i = 1, #t do o[i] = t[i] end
        return o
    end

    -- ------------------------------------------------------------------------------------------ RunService: frames
    local bound = {}
    local PRI = {RenderStepped = 1, PreRender = 2, PreAnimation = 3, Stepped = 4, PreSimulation = 5, Heartbeat = 6, PostSimulation = 7}
    local sorted, sortedn = {}, -1
    function rt.fire_frame(dt)
        local list = rt.frame_sigs
        if #list ~= sortedn then
            sorted = copy(list)
            table.sort(sorted, function(a, b) return (PRI[DATA[a].name] or 9) < (PRI[DATA[b].name] or 9) end)
            sortedn = #list
        end
        if next(bound) ~= nil then
            local items = {}
            for _, b in next, bound do items[#items + 1] = b end
            table.sort(items, function(a, b) return a.pri < b.pri end)
            for i = 1, #items do rt.spawn(items[i].fn, dt) end
        end
        local t = rt.clock()
        for i = 1, #sorted do
            local s = sorted[i]
            local sd = DATA[s]
            if #sd.conns > 0 or #sd.waiters > 0 then
                if sd.name == 'Stepped' then fire(s, t, dt) else fire(s, dt) end
            end
        end
    end
    defm('RunService', 'IsClient', function() return true end)
    defm('RunService', 'IsServer', function() return false end)
    defm('RunService', 'IsStudio', function() return false end)
    defm('RunService', 'IsRunning', function() return true end)
    defm('RunService', 'IsRunMode', function() return false end)
    defm('RunService', 'IsEdit', function() return false end)
    defm('RunService', 'BindToRenderStep', function(self, d, name, pri, fn)
        if type(fn) ~= 'function' then error('Argument 3 missing or nil', 3) end
        bound[name] = {pri = type(pri) == 'number' and pri or 0, fn = fn}
        rt.note_handler(nil, fn)
        rt.ensure_ticker()
    end)
    defm('RunService', 'UnbindFromRenderStep', function(self, d, name) bound[name] = nil end)
    defm('RunService', 'Set3dRenderingEnabled', function() end)
    defm('RunService', 'GetCoreScriptVersion', function() return '0.0.0' end)

    -- ------------------------------------------------------------------------------------------ input
    SI.UserInputService = function(u, d)
        local p = d.props
        p.MouseEnabled, p.KeyboardEnabled, p.TouchEnabled, p.GamepadEnabled = true, true, false, false
        p.MouseBehavior = E('MouseBehavior', 'Default')
        p.MouseIconEnabled, p.ModalEnabled, p.VREnabled = true, false, false
        p.AccelerometerEnabled, p.GyroscopeEnabled, p.OnScreenKeyboardVisible = false, false, false
        p.MouseDeltaSensitivity = 1
    end
    defm('UserInputService', 'IsKeyDown', function() return false end)
    defm('UserInputService', 'IsMouseButtonPressed', function() return false end)
    defm('UserInputService', 'IsGamepadButtonDown', function() return false end)
    defm('UserInputService', 'GetMouseLocation', function() return V2(960, 540) end)
    defm('UserInputService', 'GetMouseDelta', function() return V2(0, 0) end)
    defm('UserInputService', 'GetKeysPressed', function() return {} end)
    defm('UserInputService', 'GetFocusedTextBox', function() return nil end)
    defm('UserInputService', 'GetLastInputType', function() return E('UserInputType', 'Keyboard') end)
    defm('UserInputService', 'GetPlatform', function() return E('Platform', 'Windows') end)
    defm('UserInputService', 'GetConnectedGamepads', function() return {} end)
    defm('UserInputService', 'GetGamepadConnected', function() return false end)
    defm('UserInputService', 'GetNavigationGamepads', function() return {} end)
    defm('UserInputService', 'GetStringForKeyCode', function(self, d, kc) return KIND[kc] == 'EnumItem' and DATA[kc].name or '' end)
    defm('UserInputService', 'GetDeviceAcceleration', function() return V3(0, 0, 0) end)

    defm('GuiService', 'GetGuiInset', function() return V2(0, 36), V2(0, 0) end)
    defm('GuiService', 'IsTenFootInterface', function() return false end)

    defm('ContextActionService', 'BindAction', function(self, d, name, fn)
        if type(fn) == 'function' then rt.note_handler(nil, fn) end
    end)
    defm('ContextActionService', 'BindActionAtPriority', function(self, d, name, fn)
        if type(fn) == 'function' then rt.note_handler(nil, fn) end
    end)
    defm('ContextActionService', 'BindActivate', function() end)
    defm('ContextActionService', 'UnbindAction', function() end)
    defm('ContextActionService', 'GetAllBoundActionInfo', function() return {} end)

    -- ------------------------------------------------------------------------------------------ tweens
    DEF.Tween = {PlaybackState = E('PlaybackState', 'Begin')}
    defm('TweenService', 'Create', function(self, d, inst, info, goals)
        local tw, td = newinst('Tween', {name = 'Tween', world = false})
        td.tween = {inst = inst, info = info, goals = goals}
        return tw
    end)
    local function tween_time(info)
        if KIND[info] ~= 'TweenInfo' then return 1 end
        local i = DATA[info]
        local reps = i.RepeatCount or 0
        if reps < 0 then return nil end
        return (i.DelayTime or 0) + (i.Time or 1) * (reps + 1) * (i.Reverses and 2 or 1)
    end
    defm('Tween', 'Play', function(self, d)
        local tw = d.tween
        if tw == nil then return end
        local total = tween_time(tw.info)
        d.props.PlaybackState = E('PlaybackState', 'Playing')
        local token = {}
        d.token = token
        if total == nil then return end
        rt.later(total, function()
            if d.token ~= token then return end
            d.token = nil
            local inst = tw.inst
            if KIND[inst] == 'Instance' and type(tw.goals) == 'table' and not DATA[inst].destroyed then
                for k, v in next, tw.goals do
                    if type(k) == 'string' then setprop(inst, k, v) end
                end
            end
            d.props.PlaybackState = E('PlaybackState', 'Completed')
            fire_if(self, 'Completed', E('PlaybackState', 'Completed'))
        end)
    end)
    defm('Tween', 'Pause', function(self, d)
        d.token = nil
        d.props.PlaybackState = E('PlaybackState', 'Paused')
    end)
    defm('Tween', 'Cancel', function(self, d)
        if d.token ~= nil then
            d.token = nil
            d.props.PlaybackState = E('PlaybackState', 'Cancelled')
            fire_if(self, 'Completed', E('PlaybackState', 'Cancelled'))
        end
    end)

    -- ------------------------------------------------------------------------------------------ JSON
    local JESC = {['"'] = '\\"', ['\\'] = '\\\\', ['\n'] = '\\n', ['\r'] = '\\r', ['\t'] = '\\t', ['\b'] = '\\b', ['\f'] = '\\f'}
    local function jstr(s)
        return '"' .. sgsub(s, '[%c"\\]', function(c) return JESC[c] or sfmt('\\u%04x', sbyte(c)) end) .. '"'
    end
    local function array_length(t)
        local n = 0
        for k in next, t do
            if type(k) ~= 'number' or k < 1 or k ~= floor(k) then return nil end
            n = n + 1
        end
        for i = 1, n do
            if rawget(t, i) == nil then return nil end
        end
        return n
    end
    local function enc(v, depth, seen)
        local t = type(v)
        if t == 'string' then return jstr(v)
        elseif t == 'number' then
            if v ~= v or v == huge or v == -huge then error("Can't convert number to JSON", 0) end
            if v == floor(v) and v > -1e15 and v < 1e15 then return sfmt('%d', v) end
            return sfmt('%.14g', v)
        elseif t == 'boolean' then return v and 'true' or 'false'
        elseif t == 'nil' then return 'null'
        elseif t == 'userdata' and KIND[v] == 'Ghost' then return jstr(tostring(v))   -- a value the sandbox does not know: its text
        elseif t == 'table' then
            if depth > 100 or seen[v] then error("Can't convert table to JSON (cyclic or too deep)", 0) end
            seen[v] = true
            local out = {}
            local n = array_length(v)
            local res
            if n ~= nil then
                for i = 1, n do out[i] = enc(rawget(v, i), depth + 1, seen) end
                res = '[' .. tconcat(out, ',') .. ']'
            else
                for k, val in next, v do
                    if type(k) == 'string' then out[#out + 1] = jstr(k) .. ':' .. enc(val, depth + 1, seen)
                    elseif type(k) == 'number' then out[#out + 1] = jstr(rt.num(k)) .. ':' .. enc(val, depth + 1, seen)
                    else error("Can't convert key of type " .. rt.typeof(k) .. ' to JSON', 0) end
                end
                table.sort(out)
                res = '{' .. tconcat(out, ',') .. '}'
            end
            seen[v] = nil
            return res
        end
        error("Can't convert " .. rt.typeof(v) .. ' to JSON', 0)
    end
    function rt.json_encode(v) return enc(v, 0, {}) end

    local UNESC = {['"'] = '"', ['\\'] = '\\', ['/'] = '/', b = '\b', f = '\f', n = '\n', r = '\r', t = '\t'}
    function rt.json_decode(s)
        if type(s) ~= 'string' then error('Argument 1 missing or nil', 3) end
        local pos, n = 1, #s
        local function fail() error("Can't parse JSON", 0) end
        local function skip() pos = string.find(s, '[^ \t\r\n]', pos) or n + 1 end
        local function pstring()
            local out, i = {}, pos + 1
            while true do
                local j = string.find(s, '["\\]', i)
                if not j then fail() end
                out[#out + 1] = ssub(s, i, j - 1)
                if sbyte(s, j) == 34 then pos = j + 1; return tconcat(out) end
                local c = ssub(s, j + 1, j + 1)
                if c == 'u' then
                    local hex = string.match(s, '^%x%x%x%x', j + 2)
                    if not hex then fail() end
                    local cp, adv = tonumber(hex, 16), j + 6
                    if cp >= 0xD800 and cp <= 0xDBFF then
                        local lo = string.match(s, '^\\u(%x%x%x%x)', adv)
                        local l = lo and tonumber(lo, 16)
                        if l and l >= 0xDC00 and l <= 0xDFFF then cp = 0x10000 + (cp - 0xD800) * 1024 + (l - 0xDC00); adv = adv + 6 end
                    end
                    if cp >= 0xD800 and cp <= 0xDFFF then cp = 0xFFFD end
                    out[#out + 1] = utf8.char(cp)
                    i = adv
                else
                    local m = UNESC[c]
                    if not m then fail() end
                    out[#out + 1] = m
                    i = j + 2
                end
            end
        end
        local value
        local function pvalue(depth)
            if depth > 200 then fail() end
            skip()
            local c = ssub(s, pos, pos)
            if c == '{' then
                pos = pos + 1
                local o = {}
                skip()
                if ssub(s, pos, pos) == '}' then pos = pos + 1; return o end
                while true do
                    skip()
                    if ssub(s, pos, pos) ~= '"' then fail() end
                    local k = pstring()
                    skip()
                    if ssub(s, pos, pos) ~= ':' then fail() end
                    pos = pos + 1
                    o[k] = pvalue(depth + 1)
                    skip()
                    local d = ssub(s, pos, pos)
                    pos = pos + 1
                    if d == '}' then return o end
                    if d ~= ',' then fail() end
                end
            elseif c == '[' then
                pos = pos + 1
                local a, k = {}, 0
                skip()
                if ssub(s, pos, pos) == ']' then pos = pos + 1; return a end
                while true do
                    k = k + 1
                    a[k] = pvalue(depth + 1)
                    skip()
                    local d = ssub(s, pos, pos)
                    pos = pos + 1
                    if d == ']' then return a end
                    if d ~= ',' then fail() end
                end
            elseif c == '"' then
                return pstring()
            elseif ssub(s, pos, pos + 3) == 'true' then pos = pos + 4; return true
            elseif ssub(s, pos, pos + 4) == 'false' then pos = pos + 5; return false
            elseif ssub(s, pos, pos + 3) == 'null' then pos = pos + 4; return nil
            else
                local m = string.match(s, '^-?%d+%.?%d*[eE]?[+-]?%d*', pos)
                if not m or m == '' then fail() end
                local v = tonumber(m)
                if v == nil then fail() end
                pos = pos + #m
                return v
            end
        end
        local result = pvalue(0)
        skip()
        if pos <= n then fail() end
        return result
    end

    -- ------------------------------------------------------------------------------------------ HttpService
    SI.HttpService = function(u, d) d.props.HttpEnabled = true end
    local function hex(k)
        local t = {}
        for i = 1, k do t[i] = sfmt('%X', rt.rand_int(16) - 1) end
        return tconcat(t)
    end
    defm('HttpService', 'JSONEncode', function(self, d, v) return rt.json_encode(v) end, true)
    defm('HttpService', 'JSONDecode', function(self, d, s) return rt.json_decode(s) end, true)
    defm('HttpService', 'GenerateGUID', function(self, d, wrap)
        local g = hex(8) .. '-' .. hex(4) .. '-4' .. hex(3) .. '-' .. sfmt('%X', 7 + rt.rand_int(4)) .. hex(3) .. '-' .. hex(12)
        if wrap == false then return g end
        return '{' .. g .. '}'
    end, true)
    defm('HttpService', 'UrlEncode', function(self, d, s)
        return (sgsub(tostring(s), '[^%w%-_%.~]', function(c) return sfmt('%%%02X', sbyte(c)) end))
    end, true)

    -- the answer to a request made with request() or HttpService:RequestAsync
    function rt.http_response(text, url, method, body)
        local b = rt.fetch(text, url, method, body)
        local code = 200
        if string.find(url, 'discord%.com/api/webhooks/') or string.find(url, 'discordapp%.com/api/webhooks/') then code = 204 end
        return {Success = true, StatusCode = code, StatusMessage = code == 204 and 'No Content' or 'OK', Headers = {}, Body = b}
    end
    defm('HttpService', 'GetAsync', function(self, d, url)
        if type(url) ~= 'string' then error('Argument 1 missing or nil', 3) end
        return rt.fetch(ref(self) .. ':GetAsync(' .. quote(url, 300) .. ')', url, 'GET')
    end, true)
    defm('HttpService', 'PostAsync', function(self, d, url, data)
        if type(url) ~= 'string' then error('Argument 1 missing or nil', 3) end
        return rt.fetch(ref(self) .. ':PostAsync(' .. quote(url, 300) .. ', ' .. repr(data, 1) .. ')', url, 'POST', type(data) == 'string' and data or nil)
    end, true)
    defm('HttpService', 'RequestAsync', function(self, d, opts)
        if type(opts) ~= 'table' or type(opts.Url) ~= 'string' then error('Argument 1 missing or nil', 3) end
        local method = string.upper(tostring(opts.Method or 'GET'))
        local text = ref(self) .. ':RequestAsync(' .. repr(opts, 0) .. ')'
        return rt.http_response(text, opts.Url, method, type(opts.Body) == 'string' and opts.Body or nil)
    end, true)

    -- ------------------------------------------------------------------------------------------ teleports and the store
    defm('TeleportService', 'Teleport', function() end)
    defm('TeleportService', 'TeleportToPlaceInstance', function() end)
    defm('TeleportService', 'TeleportToSpawnByName', function() end)
    defm('TeleportService', 'TeleportAsync', function(self, d) return rt.ghost(self, ':TeleportAsync()') end)
    defm('TeleportService', 'TeleportPartyAsync', function(self, d) return rt.ghost(self, ':TeleportPartyAsync()') end)
    defm('TeleportService', 'ReserveServer', function() return 'access-code-0001', 'private-server-0001' end)
    defm('TeleportService', 'GetLocalPlayerTeleportData', function() return nil end)
    defm('TeleportService', 'SetTeleportGui', function() end)

    defm('MarketplaceService', 'GetProductInfo', function(self, d, id)
        local nm = CONFIG.game_name or ('Place ' .. tostring(id))
        return {Name = nm, Description = '', AssetId = id, ProductId = id, PriceInRobux = 0, IsForSale = false, IsPublicDomain = false,
            Creator = {CreatorType = 'User', CreatorTargetId = 1, Name = 'Creator', Id = 1, HasVerifiedBadge = false},
            Created = '2020-01-01T00:00:00Z', Updated = '2020-01-01T00:00:00Z', AssetTypeId = 9, ContentRatingTypeId = 0,
            MinimumMembershipLevel = 0, Remaining = 0, Sales = 0, IsLimited = false, IsLimitedUnique = false, IsNew = false,
            IconImageAssetId = 0}
    end)
    defm('MarketplaceService', 'UserOwnsGamePassAsync', function() return false end)
    defm('MarketplaceService', 'PlayerOwnsAsset', function() return false end)
    defm('MarketplaceService', 'PlayerOwnsBundle', function() return false end)
    defm('MarketplaceService', 'GetRobuxBalance', function() return 0 end)
    defm('GroupService', 'GetGroupInfoAsync', function(self, d, id)
        return {Name = 'Group', Id = id, Owner = {Name = 'Owner', Id = 1}, EmblemUrl = '', Description = '', Roles = {}}
    end)
    defm('BadgeService', 'UserHasBadgeAsync', function() return false end)
    defm('LocalizationService', 'GetCountryRegionForPlayerAsync', function() return 'US' end)
    defm('PolicyService', 'GetPolicyInfoForPlayerAsync', function() return {AreAdsAllowed = true, ArePaidRandomItemsRestricted = false,
        IsPaidItemTradingAllowed = true, IsSubjectToChinaPolicies = false} end)

    defm('StarterGui', 'SetCore', function(self, d, name, val)
        d.core = d.core or {}
        d.core[name] = val
    end)
    defm('StarterGui', 'GetCore', function(self, d, name)
        local c = d.core and d.core[name]
        if c ~= nil then return c end
        return rt.ghost(self, ':GetCore(' .. quote(tostring(name)) .. ')')
    end)
    defm('StarterGui', 'SetCoreGuiEnabled', function() end)
    defm('StarterGui', 'GetCoreGuiEnabled', function() return true end)

    DEF.StarterGui = {ShowDevelopmentGui = true}
    defm('Lighting', 'GetMinutesAfterMidnight', function(self, d) return (d.props.ClockTime or 14) * 60 end)
    defm('Lighting', 'SetMinutesAfterMidnight', function(self, d, m) d.props.ClockTime = (m or 0) / 60 end)
    defm('Lighting', 'GetSunDirection', function() return V3(0, 1, 0) end)
    defm('Lighting', 'GetMoonDirection', function() return V3(0, -1, 0) end)
    defm('SoundService', 'PlayLocalSound', function() end)

    -- ------------------------------------------------------------------------------------------ small services
    defm('Debris', 'AddItem', function(self, d, item, t)
        if KIND[item] ~= 'Instance' then return end
        rt.later(type(t) == 'number' and t or 10, function()
            if not DATA[item].destroyed then rt.destroy(item) end
        end)
    end)

    local added, removed = {}, {}
    local function tag_signal(map, tag, how)
        local s = map[tag]
        if not s then
            s = rt.newsig(nil, tostring(tag), how)
            map[tag] = s
        end
        return s
    end
    defm('CollectionService', 'GetTagged', function(self, d, tag)
        local out, set = {}, rt.tags[tag]
        if set then
            for inst in next, set do
                if not DATA[inst].destroyed then out[#out + 1] = inst end
            end
        end
        return out
    end)
    local function add_tag(self, d, a, b)
        local inst, tag = a, b
        if KIND[inst] ~= 'Instance' then return end
        rt.tags[tag] = rt.tags[tag] or {}
        if not rt.tags[tag][inst] then
            rt.tags[tag][inst] = true
            local s = added[tag]
            if s then fire(s, inst) end
        end
    end
    defm('CollectionService', 'AddTag', add_tag)
    defm('CollectionService', 'RemoveTag', function(self, d, inst, tag)
        if rt.tags[tag] and rt.tags[tag][inst] then
            rt.tags[tag][inst] = nil
            local s = removed[tag]
            if s then fire(s, inst) end
        end
    end)
    defm('CollectionService', 'HasTag', function(self, d, inst, tag) return rt.tags[tag] ~= nil and rt.tags[tag][inst] == true end)
    defm('CollectionService', 'GetTags', function(self, d, inst)
        local o = {}
        for t, set in next, rt.tags do if set[inst] then o[#o + 1] = t end end
        return o
    end)
    defm('CollectionService', 'GetInstanceAddedSignal', function(self, d, tag)
        return tag_signal(added, tag, function() return ref(self) .. ':GetInstanceAddedSignal(' .. quote(tostring(tag)) .. ')' end)
    end)
    defm('CollectionService', 'GetInstanceRemovedSignal', function(self, d, tag)
        return tag_signal(removed, tag, function() return ref(self) .. ':GetInstanceRemovedSignal(' .. quote(tostring(tag)) .. ')' end)
    end)
    -- Instance:AddTag tells the signals of the CollectionService too
    defm('Instance', 'AddTag', function(self, d, tag) add_tag(nil, nil, self, tag) end)
    defm('Instance', 'RemoveTag', function(self, d, tag)
        if rt.tags[tag] and rt.tags[tag][self] then
            rt.tags[tag][self] = nil
            local s = removed[tag]
            if s then fire(s, self) end
        end
    end)

    defm('PathfindingService', 'CreatePath', function(self, d, params)
        local p, pd = newinst('Path', {name = 'Path', world = false})
        pd.props.Status = E('PathStatus', 'Success')
        return p
    end)
    defm('Path', 'ComputeAsync', function(self, d, a, b) d.from, d.to = a, b end)
    defm('Path', 'GetWaypoints', function(self, d)
        local function wp(v)
            return {Position = v or V3(0, 0, 0), Action = E('PathWaypointAction', 'Walk'), Label = ''}
        end
        return {wp(d.from), wp(d.to)}
    end)

    defm('TextService', 'GetTextSize', function(self, d, text, size)
        local n = type(text) == 'string' and #text or 0
        size = type(size) == 'number' and size or 14
        return V2(n * size * 0.5, size)
    end, true)

    defm('Stats', 'GetTotalMemoryUsageMb', function() return 600 end)
    defm('StatsItem', 'GetValue', function() return 60 end)
    defm('StatsItem', 'GetValueString', function() return '60.00' end)
    DEF.Stats = {HeartbeatTimeMs = 16, InstanceCount = 50000, PrimitivesCount = 10000, MovingPrimitivesCount = 100, DataReceiveKbps = 10,
        DataSendKbps = 5, PhysicsReceiveKbps = 1, PhysicsSendKbps = 1}
end
