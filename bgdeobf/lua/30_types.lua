-- part 3: how fake objects are made.  Instances, signals, Vector3 and the rest are userdata (so type() says "userdata"
-- and a script can not look inside them), made by newproxy; this part holds the common machinery, the "Ghost" that
-- stands in for anything the sandbox does not know, and the Enum tables.
do
    local METAS = {}
    local EQF = {}                     -- kind -> function(data a, data b): are two values of that kind equal?
    rt.EQF = EQF

    -- one function for all kinds: Lua only calls __eq when both sides have the same one
    local function EQ(a, b)
        if rawequal(a, b) then return true end
        local ka = KIND[a]
        if ka ~= KIND[b] then return false end
        local f = EQF[ka]
        return f ~= nil and f(DATA[a], DATA[b]) or false
    end
    rt.EQ = EQ

    function rt.defkind(kind, mm)
        mm.__eq = EQ
        if mm.__metatable == nil then mm.__metatable = 'The metatable is locked' end
        METAS[kind] = mm
    end

    local function newobj(kind, data)
        local u = newproxy(true)
        local mt = getmetatable(u)
        for k, v in next, METAS[kind] do mt[k] = v end
        KIND[u] = kind
        DATA[u] = data
        return u
    end
    rt.newobj = newobj

    -- the name of the type as `typeof` says it
    function rt.typeof(v)
        local k = KIND[v]
        if k == nil then return typeof(v) end
        if k == 'Ghost' then return 'Instance' end
        return k
    end
end

rt.note_handler = function() end       -- (replaced by the exploring part)

-- ------------------------------------------------------------------------------------------------ the Ghost
-- Anything the sandbox does not know about is a Ghost: you can read a field of it, call it, add to it, loop over it, and
-- you get another Ghost.  It remembers how it was reached (`workspace.Map.Door`), so the log can show it.
do
    local newobj = rt.newobj
    local kids = weak()                -- ghost -> {key -> ghost}

    local function ghost(parent, how, key)
        return newobj('Ghost', {parent = parent, how = how, key = key})
    end
    rt.ghost = ghost
    -- a Ghost that always shows as the given expression (the result of require(...), settings(), ...)
    function rt.ghost_of(expr) return newobj('Ghost', {text = expr}) end

    local function text(g, depth)
        local d = DATA[g]
        if d.text then return d.text end
        local base = d.parent ~= nil and rt.repr(d.parent, depth + 1) or 'unknown'
        if d.how == nil then return base end
        local s = base .. d.how
        if #s > 160 then s = ssub(s, 1, 160) .. '...' end
        return s
    end
    rt.REPR.Ghost = text

    -- Luau can not compare a number with a userdata (`ghost > 0` is an error), so a field whose name says it holds a number
    -- is a number: 0.  Everything else the sandbox does not know stays a Ghost.
    local NUMERIC = {}
    for w in string.gmatch([[count amount level exp xp health maxhealth time duration cooldown remaining distance speed cost price stock
        progress score points money cash coins gold gems wave round stage room kills streak number index length width height total current
        rate chance damage power strength rank tier wins losses deaths lives timer countdown seconds minutes hours days age size max min
        id value qty quantity multiplier percent percentage amountleft left charges uses ammo energy mana stamina hunger thirst]], '%a+') do
        NUMERIC[w] = true
    end
    local NUMERIC_END = {'count', 'amount', 'level', 'time', 'cooldown', 'distance', 'speed', 'cost', 'price', 'points', 'score', 'coins',
        'kills', 'wins', 'health', 'duration', 'remaining', 'timer', 'rate', 'chance', 'damage', 'stock', 'progress', 'multiplier'}
    local function numeric_field(k)
        if type(k) ~= 'string' then return false end
        local l = string.lower(k)
        if NUMERIC[l] then return true end
        for i = 1, #NUMERIC_END do
            if #l > #NUMERIC_END[i] and ssub(l, -#NUMERIC_END[i]) == NUMERIC_END[i] then return true end
        end
        return false
    end
    rt.numeric_field = numeric_field

    local function fresh(self, how) return ghost(self, how) end
    local function dot(k)
        if type(k) == 'string' and string.find(k, '^[%a_][%w_]*$') and not rt.RESERVED[k] then return '.' .. k end
        return '[' .. rt.repr(k, 2) .. ']'
    end

    local function index(self, k)
        local c = kids[self]
        if c == nil then c = {}; kids[self] = c end
        local g = c[k]
        if g == nil then
            if k == 'Connect' or k == 'Once' or k == 'ConnectParallel' then
                g = function(me, fn, ...)
                    rt.emit('conn', text(me, 0) .. ':' .. k .. '(' .. rt.args(pack(fn, ...), select('#', ...) + 1) .. ')')
                    if type(fn) == 'function' then rt.note_handler(me, fn) end
                    return ghost(me, ':' .. k .. '()')
                end
            elseif k == 'Wait' then
                g = function(me)
                    rt.emit('call', text(me, 0) .. ':Wait()')
                    return ghost(me, ':Wait()')
                end
            elseif k == 'Disconnect' then
                g = function(me) rt.emit('call', text(me, 0) .. ':Disconnect()') end
            elseif numeric_field(k) then
                g = 0
            else
                g = ghost(self, dot(k), k)
            end
            c[k] = g
        end
        return g
    end

    local function arith(a, b) return ghost(KIND[a] == 'Ghost' and a or b, ' --[[arithmetic]]') end

    rt.defkind('Ghost', {
        __index = index,
        __newindex = function(self, k, v)
            rt.emit('set', text(self, 0) .. dot(k) .. ' = ' .. rt.repr(v, 1))
        end,
        __call = function(self, ...)
            local n = select('#', ...)
            local args = pack(...)
            local d = DATA[self]
            local call
            if d.key ~= nil and n >= 1 and rawequal(args[1], d.parent) and type(d.key) == 'string' then
                -- obj:Method(...) arrives as obj.Method(obj, ...)
                local rest = {n = n - 1, unpack(args, 2, n)}
                call = rt.repr(d.parent, 1) .. ':' .. d.key .. '(' .. rt.args(rest, n - 1) .. ')'
            else
                call = text(self, 0) .. '(' .. rt.args(args, n) .. ')'
            end
            rt.emit('call', call)
            return ghost(self, '(' .. rt.args(args, n) .. ')')
        end,
        __tostring = function(self) return text(self, 0) end,
        __add = arith, __sub = arith, __mul = arith, __div = arith, __mod = arith, __pow = arith, __idiv = arith,
        __unm = function(self) return self end,
        __concat = function(a, b)
            return (KIND[a] == 'Ghost' and text(a, 0) or tostring(a)) .. (KIND[b] == 'Ghost' and text(b, 0) or tostring(b))
        end,
        __len = function() return 0 end,
        __lt = function() return false end,
        __le = function() return false end,
        __iter = function(self) return function() return nil end, self, nil end,
    })
end

-- ------------------------------------------------------------------------------------------------ enums
do
    local newobj = rt.newobj
    local types, itemlist = {}, {}
    -- values the scripts compare against; everything else gets the next free number
    local KNOWN = {
        KeyCode = {Unknown = 0, Backspace = 8, Tab = 9, Return = 13, Escape = 27, Space = 32, Delete = 127, Zero = 48, One = 49,
            Two = 50, Three = 51, Four = 52, Five = 53, Six = 54, Seven = 55, Eight = 56, Nine = 57, A = 97, B = 98, C = 99,
            D = 100, E = 101, F = 102, G = 103, H = 104, I = 105, J = 106, K = 107, L = 108, M = 109, N = 110, O = 111, P = 112,
            Q = 113, R = 114, S = 115, T = 116, U = 117, V = 118, W = 119, X = 120, Y = 121, Z = 122, LeftShift = 304,
            RightShift = 303, LeftControl = 306, RightControl = 305, LeftAlt = 308, RightAlt = 307, Up = 273, Down = 274,
            Right = 275, Left = 276, F1 = 282, F2 = 283, F3 = 284, F4 = 285, F5 = 286, F6 = 287, F7 = 288, F8 = 289, F9 = 290,
            F10 = 291, F11 = 292, F12 = 293, Insert = 277, Home = 278, End = 279, PageUp = 280, PageDown = 281, RightBracket = 93,
            LeftBracket = 91, Backquote = 96, Minus = 45, Equals = 61, Comma = 44, Period = 46, Slash = 47, Semicolon = 59,
            Quote = 39, BackSlash = 92, ButtonA = 1000, ButtonB = 1001, ButtonX = 1002, ButtonY = 1003, Thumbstick1 = 1011,
            Thumbstick2 = 1012},
        UserInputType = {MouseButton1 = 0, MouseButton2 = 1, MouseButton3 = 2, MouseWheel = 3, MouseMovement = 4,
            Touch = 7, Keyboard = 8, Focus = 9, Accelerometer = 10, Gyro = 11, Gamepad1 = 12, TextInput = 14, None = 15},
        Material = {Plastic = 256, SmoothPlastic = 272, Neon = 288, Wood = 512, Slate = 800, Concrete = 816, Metal = 1088,
            Grass = 1280, Sand = 1296, Brick = 848, Glass = 1568, ForceField = 1584, Air = 1792, Water = 2048},
        HumanoidStateType = {FallingDown = 0, Ragdoll = 1, GettingUp = 2, Jumping = 3, Swimming = 4, Freefall = 5, Flying = 6,
            Landed = 7, Running = 8, RunningNoPhysics = 10, StrafingNoPhysics = 11, Climbing = 12, Seated = 13,
            PlatformStanding = 14, Dead = 15, Physics = 16, None = 18},
        EasingStyle = {Linear = 0, Sine = 1, Back = 2, Quad = 3, Quart = 4, Quint = 5, Bounce = 6, Elastic = 7, Exponential = 8,
            Circular = 9, Cubic = 10},
        EasingDirection = {In = 0, Out = 1, InOut = 2},
        PlaybackState = {Begin = 0, Delayed = 1, Playing = 2, Paused = 3, Completed = 4, Cancelled = 5},
        RaycastFilterType = {Exclude = 0, Include = 1, Blacklist = 0, Whitelist = 1},
        MembershipType = {None = 0, BuildersClub = 1, TurboBuildersClub = 2, OutrageousBuildersClub = 3, Premium = 4},
        Platform = {Windows = 0, OSX = 1, IOS = 2, Android = 3, XBoxOne = 4, PS4 = 5, UWP = 9, None = 11},
        CameraType = {Fixed = 0, Watch = 1, Attach = 2, Track = 3, Follow = 4, Custom = 5, Scriptable = 6, Orbital = 7},
        MouseBehavior = {Default = 0, LockCenter = 1, LockCurrentPosition = 2},
        RigType = {R6 = 0, R15 = 1},
        Font = {Legacy = 0, Arial = 1, ArialBold = 2, SourceSans = 3, SourceSansBold = 4, SourceSansLight = 5, SourceSansItalic = 6,
            Bodoni = 7, Garamond = 8, Cartoon = 9, Code = 10, Highway = 11, SciFi = 12, Arcade = 13, Fantasy = 14, Antique = 15,
            Gotham = 16, GothamBold = 17, GothamSemibold = 18, GothamBlack = 19},
    }

    local function etype(name)
        local t = types[name]
        if t then return t end
        t = newobj('EnumType', {name = name, items = {}, list = {}, next = 0})
        types[name] = t
        local known = KNOWN[name]
        if known then
            for k, v in pairs(known) do
                local d = DATA[t]
                d.next = math.max(d.next, v + 1)
            end
        end
        return t
    end

    local function item(t, key)
        local d = DATA[t]
        local it = d.items[key]
        if it then return it end
        local value
        local known = KNOWN[d.name]
        if known and known[key] ~= nil then value = known[key] else value = d.next; d.next = d.next + 1 end
        it = newobj('EnumItem', {name = key, value = value, type = t})
        d.items[key] = it
        d.list[#d.list + 1] = it
        return it
    end
    rt.enum_item = function(typename, key) return item(etype(typename), key) end

    local function allitems(t)
        local d = DATA[t]
        local known = KNOWN[d.name]
        if known then
            local names = {}
            for k in pairs(known) do names[#names + 1] = k end
            table.sort(names, function(a, b) return known[a] < known[b] end)
            for _, k in ipairs(names) do item(t, k) end
        end
        local out = {}
        for i, it in ipairs(d.list) do out[i] = it end
        return out
    end

    rt.defkind('EnumItem', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'Name' then return d.name elseif k == 'Value' then return d.value elseif k == 'EnumType' then return d.type end
            if k == 'IsA' then return function(me, n) return n == 'EnumItem' or n == DATA[d.type].name end end
            error(tostring(k) .. ' is not a valid member of EnumItem', 2)
        end,
        __tostring = function(self) return 'Enum.' .. DATA[DATA[self].type].name .. '.' .. DATA[self].name end,
    })
    rt.REPR.EnumItem = function(self) return 'Enum.' .. DATA[DATA[self].type].name .. '.' .. DATA[self].name end

    rt.defkind('EnumType', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'GetEnumItems' then return function(me) return allitems(me) end end
            if k == 'Name' then return d.name end
            if k == 'FromValue' then
                return function(me, v)
                    for _, it in ipairs(allitems(me)) do if DATA[it].value == v then return it end end
                    return nil
                end
            end
            if k == 'FromName' then return function(me, n) return item(me, n) end end
            return item(self, k)
        end,
        __tostring = function(self) return DATA[self].name end,
    })
    rt.REPR.EnumType = function(self) return 'Enum.' .. DATA[self].name end

    rt.defkind('Enums', {
        __index = function(self, k)
            if k == 'GetEnums' then
                return function() local out = {}; for _, t in pairs(types) do out[#out + 1] = t end; return out end
            end
            return etype(k)
        end,
        __tostring = function() return 'Enums' end,
    })
    rt.Enum = newobj('Enums', {})
    rt.REPR.Enums = function() return 'Enum' end
end
