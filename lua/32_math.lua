-- part 3b: the value types of Roblox: Vector3, Vector2, CFrame, Color3, UDim, UDim2, BrickColor, Random and a few
-- small ones.  They are immutable and compare by value, like the real ones.
do
    local newobj, defkind, EQF = rt.newobj, rt.defkind, rt.EQF
    local sqrt, sin, cos, atan2, asin, acos, abs, min, max = math.sqrt, math.sin, math.cos, math.atan2, math.asin, math.acos, math.abs, math.min, math.max
    local function n(x, d) if type(x) == 'number' then return x end return d end
    local function numstr(x) return rt.num(x) end

    -- ---------------------------------------------------------------- Vector3 and Vector2
    local function V3(x, y, z) return newobj('Vector3', {X = x, Y = y, Z = z}) end
    local function V2(x, y) return newobj('Vector2', {X = x, Y = y}) end
    rt.V3, rt.V2 = V3, V2
    EQF.Vector3 = function(a, b) return a.X == b.X and a.Y == b.Y and a.Z == b.Z end
    EQF.Vector2 = function(a, b) return a.X == b.X and a.Y == b.Y end

    local V3M, V2M = {}, {}
    local function d3(v) local d = DATA[v]; if KIND[v] ~= 'Vector3' then error('Vector3 expected, got ' .. typeof(v), 3) end return d end
    local function d2(v) local d = DATA[v]; if KIND[v] ~= 'Vector2' then error('Vector2 expected, got ' .. typeof(v), 3) end return d end
    function V3M.Dot(a, b) local p, q = d3(a), d3(b); return p.X * q.X + p.Y * q.Y + p.Z * q.Z end
    function V3M.Cross(a, b) local p, q = d3(a), d3(b); return V3(p.Y * q.Z - p.Z * q.Y, p.Z * q.X - p.X * q.Z, p.X * q.Y - p.Y * q.X) end
    function V3M.Lerp(a, b, t) local p, q = d3(a), d3(b); return V3(p.X + (q.X - p.X) * t, p.Y + (q.Y - p.Y) * t, p.Z + (q.Z - p.Z) * t) end
    function V3M.Angle(a, b)
        local p, q = d3(a), d3(b)
        local m = sqrt(p.X ^ 2 + p.Y ^ 2 + p.Z ^ 2) * sqrt(q.X ^ 2 + q.Y ^ 2 + q.Z ^ 2)
        if m == 0 then return 0 end
        return acos(max(-1, min(1, (p.X * q.X + p.Y * q.Y + p.Z * q.Z) / m)))
    end
    function V3M.FuzzyEq(a, b, e) local p, q = d3(a), d3(b); e = e or 1e-5; return abs(p.X - q.X) <= e and abs(p.Y - q.Y) <= e and abs(p.Z - q.Z) <= e end
    function V3M.Max(a, ...) local p = d3(a); local x, y, z = p.X, p.Y, p.Z; for _, o in ipairs({...}) do local q = d3(o); x, y, z = max(x, q.X), max(y, q.Y), max(z, q.Z) end return V3(x, y, z) end
    function V3M.Min(a, ...) local p = d3(a); local x, y, z = p.X, p.Y, p.Z; for _, o in ipairs({...}) do local q = d3(o); x, y, z = min(x, q.X), min(y, q.Y), min(z, q.Z) end return V3(x, y, z) end
    local function unit3(p) local m = sqrt(p.X ^ 2 + p.Y ^ 2 + p.Z ^ 2); if m == 0 then return V3(0, 0, 0) end return V3(p.X / m, p.Y / m, p.Z / m) end
    local V3P = {
        Magnitude = function(p) return sqrt(p.X ^ 2 + p.Y ^ 2 + p.Z ^ 2) end,
        Unit = unit3,
        x = function(p) return p.X end, y = function(p) return p.Y end, z = function(p) return p.Z end,
    }
    function V2M.Dot(a, b) local p, q = d2(a), d2(b); return p.X * q.X + p.Y * q.Y end
    function V2M.Cross(a, b) local p, q = d2(a), d2(b); return p.X * q.Y - p.Y * q.X end
    function V2M.Lerp(a, b, t) local p, q = d2(a), d2(b); return V2(p.X + (q.X - p.X) * t, p.Y + (q.Y - p.Y) * t) end
    function V2M.FuzzyEq(a, b, e) local p, q = d2(a), d2(b); e = e or 1e-5; return abs(p.X - q.X) <= e and abs(p.Y - q.Y) <= e end
    function V2M.Max(a, b) local p, q = d2(a), d2(b); return V2(max(p.X, q.X), max(p.Y, q.Y)) end
    function V2M.Min(a, b) local p, q = d2(a), d2(b); return V2(min(p.X, q.X), min(p.Y, q.Y)) end
    local V2P = {
        Magnitude = function(p) return sqrt(p.X ^ 2 + p.Y ^ 2) end,
        Unit = function(p) local m = sqrt(p.X ^ 2 + p.Y ^ 2); if m == 0 then return V2(0, 0) end return V2(p.X / m, p.Y / m) end,
        x = function(p) return p.X end, y = function(p) return p.Y end,
    }

    local function opfail(op, a, b) error('attempt to perform arithmetic (' .. op .. ') on ' .. rt.typeof(a) .. ' and ' .. rt.typeof(b), 3) end
    local function vecop(kind, make, fields)
        local function each(a, b, f)
            local out = {}
            for i, k in ipairs(fields) do out[i] = f(a[k], b[k]) end
            return make(unpack(out))
        end
        local function pair(op, f, a, b)
            local ka, kb = KIND[a], KIND[b]
            if ka == kind and kb == kind then return each(DATA[a], DATA[b], f) end
            if ka == kind and type(b) == 'number' then
                local da, out = DATA[a], {}
                for i, k in ipairs(fields) do out[i] = f(da[k], b) end
                return make(unpack(out))
            end
            if kb == kind and type(a) == 'number' then
                local db, out = DATA[b], {}
                for i, k in ipairs(fields) do out[i] = f(a, db[k]) end
                return make(unpack(out))
            end
            opfail(op, a, b)
        end
        return {
            __add = function(a, b) if KIND[a] == kind and KIND[b] == kind then return pair('add', function(x, y) return x + y end, a, b) end opfail('add', a, b) end,
            __sub = function(a, b) if KIND[a] == kind and KIND[b] == kind then return pair('sub', function(x, y) return x - y end, a, b) end opfail('sub', a, b) end,
            __mul = function(a, b) return pair('mul', function(x, y) return x * y end, a, b) end,
            __div = function(a, b) return pair('div', function(x, y) return x / y end, a, b) end,
            __idiv = function(a, b) return pair('idiv', function(x, y) return floor(x / y) end, a, b) end,
            __unm = function(a) local da, out = DATA[a], {}; for i, k in ipairs(fields) do out[i] = -da[k] end return make(unpack(out)) end,
        }
    end
    local v3ops, v2ops = vecop('Vector3', V3, {'X', 'Y', 'Z'}), vecop('Vector2', V2, {'X', 'Y'})
    local function vecmeta(kind, M, P, ops, show)
        local mm = {
            __index = function(self, k)
                local d = DATA[self]
                local v = d[k]
                if v ~= nil then return v end
                local p = P[k]
                if p then return p(d) end
                local m = M[k]
                if m then return m end
                error(tostring(k) .. ' is not a valid member of ' .. kind, 2)
            end,
            __tostring = show,
        }
        for k, f in pairs(ops) do mm[k] = f end
        defkind(kind, mm)
    end
    vecmeta('Vector3', V3M, V3P, v3ops, function(self) local d = DATA[self]; return tostring(d.X) .. ', ' .. tostring(d.Y) .. ', ' .. tostring(d.Z) end)
    vecmeta('Vector2', V2M, V2P, v2ops, function(self) local d = DATA[self]; return tostring(d.X) .. ', ' .. tostring(d.Y) end)
    rt.REPR.Vector3 = function(self) local d = DATA[self]; return 'Vector3.new(' .. numstr(d.X) .. ', ' .. numstr(d.Y) .. ', ' .. numstr(d.Z) .. ')' end
    rt.REPR.Vector2 = function(self) local d = DATA[self]; return 'Vector2.new(' .. numstr(d.X) .. ', ' .. numstr(d.Y) .. ')' end

    local Vector3 = {
        new = function(x, y, z) return V3(n(x, 0), n(y, 0), n(z, 0)) end,
        zero = V3(0, 0, 0), one = V3(1, 1, 1), xAxis = V3(1, 0, 0), yAxis = V3(0, 1, 0), zAxis = V3(0, 0, 1),
        FromNormalId = function() return V3(0, 1, 0) end,
        FromAxis = function() return V3(0, 1, 0) end,
    }
    local Vector2 = {
        new = function(x, y) return V2(n(x, 0), n(y, 0)) end,
        zero = V2(0, 0), one = V2(1, 1), xAxis = V2(1, 0), yAxis = V2(0, 1),
    }
    rt.Vector3, rt.Vector2 = Vector3, Vector2

    -- ---------------------------------------------------------------- CFrame (position + a 3x3 rotation)
    -- data: p = {x, y, z}, r = {r00, r01, r02, r10, r11, r12, r20, r21, r22}  (row i, column j: r[3 * i + j + 1])
    local function CF(x, y, z, r) return newobj('CFrame', {x = x, y = y, z = z, r = r or {1, 0, 0, 0, 1, 0, 0, 0, 1}}) end
    rt.CF = CF
    EQF.CFrame = function(a, b)
        if a.x ~= b.x or a.y ~= b.y or a.z ~= b.z then return false end
        for i = 1, 9 do if a.r[i] ~= b.r[i] then return false end end
        return true
    end
    local function matmul(a, b)
        local o = {}
        for i = 0, 2 do
            for j = 0, 2 do
                o[3 * i + j + 1] = a[3 * i + 1] * b[j + 1] + a[3 * i + 2] * b[3 + j + 1] + a[3 * i + 3] * b[6 + j + 1]
            end
        end
        return o
    end
    local function rx(a) local c, s = cos(a), sin(a); return {1, 0, 0, 0, c, -s, 0, s, c} end
    local function ry(a) local c, s = cos(a), sin(a); return {c, 0, s, 0, 1, 0, -s, 0, c} end
    local function rz(a) local c, s = cos(a), sin(a); return {c, -s, 0, s, c, 0, 0, 0, 1} end
    local function dcf(c) if KIND[c] ~= 'CFrame' then error('CFrame expected, got ' .. typeof(c), 3) end return DATA[c] end
    local function xform(d, v) local r = d.r; return d.x + r[1] * v.X + r[2] * v.Y + r[3] * v.Z, d.y + r[4] * v.X + r[5] * v.Y + r[6] * v.Z, d.z + r[7] * v.X + r[8] * v.Y + r[9] * v.Z end
    local function rotv(d, v) local r = d.r; return r[1] * v.X + r[2] * v.Y + r[3] * v.Z, r[4] * v.X + r[5] * v.Y + r[6] * v.Z, r[7] * v.X + r[8] * v.Y + r[9] * v.Z end
    local function inv(d)
        local r = d.r
        local t = {r[1], r[4], r[7], r[2], r[5], r[8], r[3], r[6], r[9]}
        return {x = -(t[1] * d.x + t[2] * d.y + t[3] * d.z), y = -(t[4] * d.x + t[5] * d.y + t[6] * d.z), z = -(t[7] * d.x + t[8] * d.y + t[9] * d.z), r = t}
    end
    local function lookat(ex, ey, ez, tx, ty, tz, up)
        local lx, ly, lz = tx - ex, ty - ey, tz - ez
        local m = sqrt(lx * lx + ly * ly + lz * lz)
        if m == 0 then return CF(ex, ey, ez) end
        lx, ly, lz = lx / m, ly / m, lz / m
        local ux, uy, uz = 0, 1, 0
        if up then local u = DATA[up]; ux, uy, uz = u.X, u.Y, u.Z end
        local px, py, pz = ly * uz - lz * uy, lz * ux - lx * uz, lx * uy - ly * ux       -- right = look x up
        local pm = sqrt(px * px + py * py + pz * pz)
        if pm < 1e-9 then px, py, pz, pm = 1, 0, 0, 1 end
        px, py, pz = px / pm, py / pm, pz / pm
        local qx, qy, qz = py * lz - pz * ly, pz * lx - px * lz, px * ly - py * lx       -- up = right x look
        return CF(ex, ey, ez, {px, qx, -lx, py, qy, -ly, pz, qz, -lz})
    end
    local function quat(x, y, z, qx, qy, qz, qw)
        local m = sqrt(qx * qx + qy * qy + qz * qz + qw * qw)
        if m == 0 then return CF(x, y, z) end
        qx, qy, qz, qw = qx / m, qy / m, qz / m, qw / m
        return CF(x, y, z, {1 - 2 * (qy * qy + qz * qz), 2 * (qx * qy - qz * qw), 2 * (qx * qz + qy * qw),
            2 * (qx * qy + qz * qw), 1 - 2 * (qx * qx + qz * qz), 2 * (qy * qz - qx * qw),
            2 * (qx * qz - qy * qw), 2 * (qy * qz + qx * qw), 1 - 2 * (qx * qx + qy * qy)})
    end
    local CFM = {}
    function CFM.Inverse(c) local i = inv(dcf(c)); return CF(i.x, i.y, i.z, i.r) end
    function CFM.ToWorldSpace(a, b) local p, q = dcf(a), dcf(b); local x, y, z = xform(p, {X = q.x, Y = q.y, Z = q.z}); return CF(x, y, z, matmul(p.r, q.r)) end
    function CFM.ToObjectSpace(a, b) local i = inv(dcf(a)); local q = dcf(b); local x, y, z = xform(i, {X = q.x, Y = q.y, Z = q.z}); return CF(x, y, z, matmul(i.r, q.r)) end
    function CFM.PointToWorldSpace(a, v) return V3(xform(dcf(a), DATA[v])) end
    function CFM.PointToObjectSpace(a, v) return V3(xform(inv(dcf(a)), DATA[v])) end
    function CFM.VectorToWorldSpace(a, v) return V3(rotv(dcf(a), DATA[v])) end
    function CFM.VectorToObjectSpace(a, v) return V3(rotv(inv(dcf(a)), DATA[v])) end
    function CFM.GetComponents(c) local d = dcf(c); local r = d.r; return d.x, d.y, d.z, r[1], r[2], r[3], r[4], r[5], r[6], r[7], r[8], r[9] end
    CFM.components = CFM.GetComponents
    function CFM.Lerp(a, b, t) local p, q = dcf(a), dcf(b); return CF(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t, p.z + (q.z - p.z) * t, p.r) end
    function CFM.ToEulerAnglesXYZ(c) local r = dcf(c).r; return atan2(-r[6], r[9]), asin(max(-1, min(1, r[3]))), atan2(-r[2], r[1]) end
    CFM.ToEulerAngles = CFM.ToEulerAnglesXYZ
    function CFM.ToOrientation(c) local r = dcf(c).r; return asin(max(-1, min(1, -r[6]))), atan2(r[3], r[9]), atan2(r[4], r[5]) end
    function CFM.ToAxisAngle() return V3(0, 1, 0), 0 end
    function CFM.FuzzyEq(a, b, e) local p, q = dcf(a), dcf(b); e = e or 1e-5; return abs(p.x - q.x) <= e and abs(p.y - q.y) <= e and abs(p.z - q.z) <= e end
    function CFM.Orthonormalize(c) return c end
    local CFP = {
        Position = function(d) return V3(d.x, d.y, d.z) end,
        X = function(d) return d.x end, Y = function(d) return d.y end, Z = function(d) return d.z end,
        p = function(d) return V3(d.x, d.y, d.z) end,
        LookVector = function(d) return V3(-d.r[3], -d.r[6], -d.r[9]) end,
        RightVector = function(d) return V3(d.r[1], d.r[4], d.r[7]) end,
        UpVector = function(d) return V3(d.r[2], d.r[5], d.r[8]) end,
        Rotation = function(d) return CF(0, 0, 0, d.r) end,
    }
    local function cfmul(a, b)
        if KIND[a] == 'CFrame' and KIND[b] == 'CFrame' then
            local p, q = DATA[a], DATA[b]
            local x, y, z = xform(p, {X = q.x, Y = q.y, Z = q.z})
            return CF(x, y, z, matmul(p.r, q.r))
        elseif KIND[a] == 'CFrame' and KIND[b] == 'Vector3' then
            return V3(xform(DATA[a], DATA[b]))
        end
        opfail('mul', a, b)
    end
    defkind('CFrame', {
        __index = function(self, k)
            local d = DATA[self]
            local p = CFP[k]
            if p then return p(d) end
            local m = CFM[k]
            if m then return m end
            error(tostring(k) .. ' is not a valid member of CFrame', 2)
        end,
        __mul = cfmul,
        __add = function(a, b)
            if KIND[a] == 'CFrame' and KIND[b] == 'Vector3' then local d, v = DATA[a], DATA[b]; return CF(d.x + v.X, d.y + v.Y, d.z + v.Z, d.r) end
            opfail('add', a, b)
        end,
        __sub = function(a, b)
            if KIND[a] == 'CFrame' and KIND[b] == 'Vector3' then local d, v = DATA[a], DATA[b]; return CF(d.x - v.X, d.y - v.Y, d.z - v.Z, d.r) end
            opfail('sub', a, b)
        end,
        __tostring = function(self) local d = DATA[self]; local r = d.r
            return tostring(d.x) .. ', ' .. tostring(d.y) .. ', ' .. tostring(d.z) .. ', ' .. tostring(r[1]) .. ', ' .. tostring(r[2]) .. ', ' .. tostring(r[3])
                .. ', ' .. tostring(r[4]) .. ', ' .. tostring(r[5]) .. ', ' .. tostring(r[6]) .. ', ' .. tostring(r[7]) .. ', ' .. tostring(r[8]) .. ', ' .. tostring(r[9]) end,
    })
    local IDENT = {1, 0, 0, 0, 1, 0, 0, 0, 1}
    rt.REPR.CFrame = function(self)
        local d = DATA[self]
        local ident = true
        for i = 1, 9 do if abs(d.r[i] - IDENT[i]) > 1e-9 then ident = false end end
        if ident then return 'CFrame.new(' .. numstr(d.x) .. ', ' .. numstr(d.y) .. ', ' .. numstr(d.z) .. ')' end
        local parts = {numstr(d.x), numstr(d.y), numstr(d.z)}
        for i = 1, 9 do parts[#parts + 1] = numstr(d.r[i]) end
        return 'CFrame.new(' .. tconcat(parts, ', ') .. ')'
    end
    local CFrame = {
        new = function(a, b, c, qx, qy, qz, qw, ...)
            if a == nil then return CF(0, 0, 0) end
            if KIND[a] == 'Vector3' then
                local p = DATA[a]
                if KIND[b] == 'Vector3' then return lookat(p.X, p.Y, p.Z, DATA[b].X, DATA[b].Y, DATA[b].Z) end
                return CF(p.X, p.Y, p.Z)
            end
            if qw ~= nil then
                local r = pack(qx, qy, qz, qw, ...)
                if r.n >= 9 then return CF(a, b, c, {qx, qy, qz, qw, ...}) end
                return quat(a, b, c, qx, qy, qz, qw)
            end
            return CF(n(a, 0), n(b, 0), n(c, 0))
        end,
        Angles = function(x, y, z) return CF(0, 0, 0, matmul(matmul(rx(n(x, 0)), ry(n(y, 0))), rz(n(z, 0)))) end,
        fromEulerAnglesXYZ = function(x, y, z) return CF(0, 0, 0, matmul(matmul(rx(n(x, 0)), ry(n(y, 0))), rz(n(z, 0)))) end,
        fromEulerAnglesYXZ = function(x, y, z) return CF(0, 0, 0, matmul(matmul(ry(n(y, 0)), rx(n(x, 0))), rz(n(z, 0)))) end,
        fromOrientation = function(x, y, z) return CF(0, 0, 0, matmul(matmul(ry(n(y, 0)), rx(n(x, 0))), rz(n(z, 0)))) end,
        lookAt = function(at, target, up) local a, t = DATA[at], DATA[target]; return lookat(a.X, a.Y, a.Z, t.X, t.Y, t.Z, up) end,
        fromAxisAngle = function(axis, angle)
            local a = DATA[axis]
            local m = sqrt(a.X ^ 2 + a.Y ^ 2 + a.Z ^ 2)
            if m == 0 then return CF(0, 0, 0) end
            local x, y, z, c, s = a.X / m, a.Y / m, a.Z / m, cos(angle), sin(angle)
            local t = 1 - c
            return CF(0, 0, 0, {t * x * x + c, t * x * y - s * z, t * x * z + s * y, t * x * y + s * z, t * y * y + c, t * y * z - s * x,
                t * x * z - s * y, t * y * z + s * x, t * z * z + c})
        end,
        fromMatrix = function(pos, vx, vy, vz)
            local p, a, b = DATA[pos], DATA[vx], DATA[vy]
            local c = vz and DATA[vz]
            if not c then c = {X = a.Y * b.Z - a.Z * b.Y, Y = a.Z * b.X - a.X * b.Z, Z = a.X * b.Y - a.Y * b.X} end
            return CF(p.X, p.Y, p.Z, {a.X, b.X, c.X, a.Y, b.Y, c.Y, a.Z, b.Z, c.Z})
        end,
    }
    CFrame.identity = CF(0, 0, 0)
    rt.CFrame = CFrame

    -- ---------------------------------------------------------------- Color3
    local function C3(r, g, b) return newobj('Color3', {R = r, G = g, B = b}) end
    rt.C3 = C3
    EQF.Color3 = function(a, b) return a.R == b.R and a.G == b.G and a.B == b.B end
    local function hsv(h, s, v)
        h = h % 1
        local i = floor(h * 6)
        local f = h * 6 - i
        local p, q, t = v * (1 - s), v * (1 - f * s), v * (1 - (1 - f) * s)
        i = i % 6
        if i == 0 then return v, t, p elseif i == 1 then return q, v, p elseif i == 2 then return p, v, t
        elseif i == 3 then return p, q, v elseif i == 4 then return t, p, v end
        return v, p, q
    end
    local C3M = {
        ToHSV = function(c)
            local d = DATA[c]
            local mx, mn = max(d.R, d.G, d.B), min(d.R, d.G, d.B)
            local dl = mx - mn
            local h = 0
            if dl > 0 then
                if mx == d.R then h = ((d.G - d.B) / dl) % 6 elseif mx == d.G then h = (d.B - d.R) / dl + 2 else h = (d.R - d.G) / dl + 4 end
                h = h / 6
            end
            return h, mx == 0 and 0 or dl / mx, mx
        end,
        ToHex = function(c) local d = DATA[c]; return sfmt('%02X%02X%02X', floor(d.R * 255 + 0.5), floor(d.G * 255 + 0.5), floor(d.B * 255 + 0.5)) end,
        Lerp = function(a, b, t) local p, q = DATA[a], DATA[b]; return C3(p.R + (q.R - p.R) * t, p.G + (q.G - p.G) * t, p.B + (q.B - p.B) * t) end,
    }
    defkind('Color3', {
        __index = function(self, k)
            local d = DATA[self]
            local v = d[k]
            if v ~= nil then return v end
            if k == 'r' then return d.R elseif k == 'g' then return d.G elseif k == 'b' then return d.B end
            local m = C3M[k]
            if m then return m end
            error(tostring(k) .. ' is not a valid member of Color3', 2)
        end,
        __tostring = function(self) local d = DATA[self]; return tostring(d.R) .. ', ' .. tostring(d.G) .. ', ' .. tostring(d.B) end,
    })
    rt.REPR.Color3 = function(self)
        local d = DATA[self]
        local r, g, b = d.R * 255, d.G * 255, d.B * 255
        if r == floor(r) and g == floor(g) and b == floor(b) then return 'Color3.fromRGB(' .. numstr(r) .. ', ' .. numstr(g) .. ', ' .. numstr(b) .. ')' end
        return 'Color3.new(' .. numstr(d.R) .. ', ' .. numstr(d.G) .. ', ' .. numstr(d.B) .. ')'
    end
    local function clamp01(x) return max(0, min(1, x)) end
    local Color3 = {
        new = function(r, g, b) return C3(clamp01(n(r, 0)), clamp01(n(g, 0)), clamp01(n(b, 0))) end,
        fromRGB = function(r, g, b) return C3(clamp01(n(r, 0) / 255), clamp01(n(g, 0) / 255), clamp01(n(b, 0) / 255)) end,
        fromHSV = function(h, s, v) return C3(hsv(n(h, 0), n(s, 0), n(v, 0))) end,
        fromHex = function(s)
            s = tostring(s):gsub('^#', '')
            local r, g, b = tonumber(ssub(s, 1, 2), 16), tonumber(ssub(s, 3, 4), 16), tonumber(ssub(s, 5, 6), 16)
            if not (r and g and b) then error('Unable to parse hex code: ' .. s, 2) end
            return C3(r / 255, g / 255, b / 255)
        end,
        toHSV = function(c) return C3M.ToHSV(c) end,
    }
    rt.Color3 = Color3

    -- ---------------------------------------------------------------- UDim, UDim2
    local function UD(s, o) return newobj('UDim', {Scale = s, Offset = o}) end
    local function UD2(xs, xo, ys, yo) return newobj('UDim2', {xs = xs, xo = xo, ys = ys, yo = yo}) end
    EQF.UDim = function(a, b) return a.Scale == b.Scale and a.Offset == b.Offset end
    EQF.UDim2 = function(a, b) return a.xs == b.xs and a.xo == b.xo and a.ys == b.ys and a.yo == b.yo end
    defkind('UDim', {
        __index = function(self, k)
            local d = DATA[self]
            if d[k] ~= nil then return d[k] end
            error(tostring(k) .. ' is not a valid member of UDim', 2)
        end,
        __add = function(a, b) return UD(DATA[a].Scale + DATA[b].Scale, DATA[a].Offset + DATA[b].Offset) end,
        __sub = function(a, b) return UD(DATA[a].Scale - DATA[b].Scale, DATA[a].Offset - DATA[b].Offset) end,
        __tostring = function(self) local d = DATA[self]; return tostring(d.Scale) .. ', ' .. tostring(d.Offset) end,
    })
    rt.REPR.UDim = function(self) local d = DATA[self]; return 'UDim.new(' .. numstr(d.Scale) .. ', ' .. numstr(d.Offset) .. ')' end
    local U2M = {
        Lerp = function(a, b, t) local p, q = DATA[a], DATA[b]; return UD2(p.xs + (q.xs - p.xs) * t, p.xo + (q.xo - p.xo) * t, p.ys + (q.ys - p.ys) * t, p.yo + (q.yo - p.yo) * t) end,
    }
    defkind('UDim2', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'X' then return UD(d.xs, d.xo) elseif k == 'Y' then return UD(d.ys, d.yo)
            elseif k == 'Width' then return UD(d.xs, d.xo) elseif k == 'Height' then return UD(d.ys, d.yo) end
            local m = U2M[k]
            if m then return m end
            error(tostring(k) .. ' is not a valid member of UDim2', 2)
        end,
        __add = function(a, b) local p, q = DATA[a], DATA[b]; return UD2(p.xs + q.xs, p.xo + q.xo, p.ys + q.ys, p.yo + q.yo) end,
        __sub = function(a, b) local p, q = DATA[a], DATA[b]; return UD2(p.xs - q.xs, p.xo - q.xo, p.ys - q.ys, p.yo - q.yo) end,
        __tostring = function(self) local d = DATA[self]; return '{' .. tostring(d.xs) .. ', ' .. tostring(d.xo) .. '}, {' .. tostring(d.ys) .. ', ' .. tostring(d.yo) .. '}' end,
    })
    rt.REPR.UDim2 = function(self)
        local d = DATA[self]
        if d.xo == 0 and d.yo == 0 then return 'UDim2.fromScale(' .. numstr(d.xs) .. ', ' .. numstr(d.ys) .. ')' end
        if d.xs == 0 and d.ys == 0 then return 'UDim2.fromOffset(' .. numstr(d.xo) .. ', ' .. numstr(d.yo) .. ')' end
        return 'UDim2.new(' .. numstr(d.xs) .. ', ' .. numstr(d.xo) .. ', ' .. numstr(d.ys) .. ', ' .. numstr(d.yo) .. ')'
    end
    rt.UDim = {new = function(s, o) return UD(n(s, 0), n(o, 0)) end}
    rt.UDim2 = {
        new = function(a, b, c, d)
            if KIND[a] == 'UDim' then local p, q = DATA[a], DATA[b] or {Scale = 0, Offset = 0}; return UD2(p.Scale, p.Offset, q.Scale, q.Offset) end
            return UD2(n(a, 0), n(b, 0), n(c, 0), n(d, 0))
        end,
        fromScale = function(x, y) return UD2(n(x, 0), 0, n(y, 0), 0) end,
        fromOffset = function(x, y) return UD2(0, n(x, 0), 0, n(y, 0)) end,
    }
    rt.UD, rt.UD2 = UD, UD2

    -- ---------------------------------------------------------------- BrickColor (the common colours only)
    local PAL = {
        {1, 'White', 242, 243, 243}, {26, 'Black', 27, 42, 53}, {21, 'Bright red', 196, 40, 28}, {23, 'Bright blue', 13, 105, 172},
        {24, 'Bright yellow', 245, 205, 48}, {37, 'Bright green', 75, 151, 75}, {106, 'Bright orange', 218, 133, 65},
        {104, 'Bright violet', 107, 50, 124}, {194, 'Medium stone grey', 163, 162, 165}, {199, 'Dark stone grey', 99, 95, 98},
        {5, 'Brick yellow', 215, 197, 154}, {1001, 'Institutional white', 248, 248, 248}, {1003, 'Really black', 17, 17, 17},
        {1004, 'Really red', 255, 0, 0}, {1010, 'Really blue', 0, 0, 255}, {1020, 'Lime green', 0, 255, 0}, {1009, 'New Yeller', 255, 255, 0},
        {1017, 'Deep orange', 255, 175, 0}, {1011, 'Navy blue', 0, 32, 96}, {1032, 'Hot pink', 255, 0, 191}, {194, 'Medium stone grey', 163, 162, 165},
        {1002, 'Light stone grey', 229, 228, 223}, {1018, 'Teal', 18, 238, 212}, {1019, 'Toothpaste', 0, 255, 255}, {1016, 'Pink', 255, 102, 204},
        {1008, 'Camo', 58, 125, 21}, {217, 'Brown', 124, 92, 70}, {38, 'Dark orange', 160, 95, 53}, {45, 'Light blue', 180, 210, 228},
    }
    local BYNAME, BYNUM = {}, {}
    for _, p in ipairs(PAL) do BYNAME[string.lower(p[2])] = p; BYNUM[p[1]] = p end
    local function BC(p) return newobj('BrickColor', {num = p[1], name = p[2], r = p[3], g = p[4], b = p[5]}) end
    EQF.BrickColor = function(a, b) return a.num == b.num end
    local function nearest(r, g, b)
        local best, bd = PAL[1], math.huge
        for _, p in ipairs(PAL) do
            local dd = (p[3] - r) ^ 2 + (p[4] - g) ^ 2 + (p[5] - b) ^ 2
            if dd < bd then best, bd = p, dd end
        end
        return best
    end
    defkind('BrickColor', {
        __index = function(self, k)
            local d = DATA[self]
            if k == 'Name' then return d.name elseif k == 'Number' then return d.num elseif k == 'Color' then return C3(d.r / 255, d.g / 255, d.b / 255)
            elseif k == 'r' then return d.r / 255 elseif k == 'g' then return d.g / 255 elseif k == 'b' then return d.b / 255 end
            error(tostring(k) .. ' is not a valid member of BrickColor', 2)
        end,
        __tostring = function(self) return DATA[self].name end,
    })
    rt.REPR.BrickColor = function(self) return 'BrickColor.new(' .. rt.quote(DATA[self].name) .. ')' end
    local function mkbrick(a, b, c)
        if type(a) == 'string' then return BC(BYNAME[string.lower(a)] or PAL[9]) end
        if KIND[a] == 'Color3' then local d = DATA[a]; return BC(nearest(d.R * 255, d.G * 255, d.B * 255)) end
        if type(a) == 'number' and b == nil then return BC(BYNUM[a] or PAL[9]) end
        if type(a) == 'number' then return BC(nearest(a * 255, n(b, 0) * 255, n(c, 0) * 255)) end
        return BC(PAL[9])
    end
    rt.BrickColor = {new = mkbrick, Random = function() return BC(PAL[1 + (rt.rand_int(#PAL) - 1)]) end,
        palette = function() return {} end}
    for _, nm in ipairs({'White', 'Black', 'Red', 'Blue', 'Yellow', 'Green', 'Gray', 'Orange'}) do
        local full = ({White = 'White', Black = 'Black', Red = 'Bright red', Blue = 'Bright blue', Yellow = 'Bright yellow', Green = 'Bright green',
            Gray = 'Medium stone grey', Orange = 'Bright orange'})[nm]
        rt.BrickColor[nm] = function() return BC(BYNAME[string.lower(full)]) end
    end

    -- ---------------------------------------------------------------- Random
    do
        local function lcg(st) st[1] = (st[1] * 1103515245 + 12345) % 2147483648; return st[1] / 2147483648 end
        rt.rand_state = {CONFIG.seed or 12345}
        function rt.rand_int(n_) return 1 + floor(lcg(rt.rand_state) * n_) end
        local RM = {
            NextNumber = function(self, a, b) local d = DATA[self]; local x = lcg(d.st); if a ~= nil then return a + x * ((b or a) - a) end return x end,
            NextInteger = function(self, a, b) local d = DATA[self]; if b == nil then a, b = 1, a end return a + floor(lcg(d.st) * (b - a + 1)) end,
            NextUnitVector = function(self) local d = DATA[self]; local z = lcg(d.st) * 2 - 1; local t = lcg(d.st) * 2 * math.pi; local r = sqrt(1 - z * z); return V3(r * cos(t), r * sin(t), z) end,
            Shuffle = function(self, t) local d = DATA[self]; for i = #t, 2, -1 do local j = 1 + floor(lcg(d.st) * i); t[i], t[j] = t[j], t[i] end end,
            Clone = function(self) return rt.Random.new(DATA[self].st[1]) end,
        }
        defkind('Random', {__index = function(self, k) local m = RM[k]; if m then return m end error(tostring(k) .. ' is not a valid member of Random', 2) end,
            __tostring = function() return 'Random' end})
        rt.REPR.Random = function() return 'Random.new()' end
        rt.Random = {new = function(seed) return newobj('Random', {st = {floor(n(seed, 1)) % 2147483648}}) end}
    end

    -- ---------------------------------------------------------------- small value bags (fields readable by name)
    local function bag(kind, fields, show)
        defkind(kind, {
            __index = function(self, k)
                local d = DATA[self]
                local v = d[k]
                if v ~= nil then return v end
                if d.__methods and d.__methods[k] then return d.__methods[k] end
                if d.__mutable then return nil end
                error(tostring(k) .. ' is not a valid member of ' .. kind, 2)
            end,
            __newindex = function(self, k, v)
                local d = DATA[self]
                if d.__mutable then d[k] = v else error('Unable to assign property ' .. tostring(k) .. '. Property is read only', 2) end
            end,
            __tostring = show or function() return kind end,
        })
        rt.REPR[kind] = function(self)
            local d = DATA[self]
            local parts = {}
            for _, f in ipairs(fields) do parts[#parts + 1] = rt.repr(d[f], 2) end
            return kind .. '.new(' .. tconcat(parts, ', ') .. ')'
        end
    end
    bag('TweenInfo', {'Time', 'EasingStyle', 'EasingDirection', 'RepeatCount', 'Reverses', 'DelayTime'})
    rt.TweenInfo = {new = function(t, st, dir, rc, rev, dl)
        return newobj('TweenInfo', {Time = n(t, 1), EasingStyle = st or rt.enum_item('EasingStyle', 'Quad'),
            EasingDirection = dir or rt.enum_item('EasingDirection', 'Out'), RepeatCount = n(rc, 0), Reverses = rev or false, DelayTime = n(dl, 0)})
    end}
    bag('NumberRange', {'Min', 'Max'})
    rt.NumberRange = {new = function(a, b) return newobj('NumberRange', {Min = n(a, 0), Max = n(b, n(a, 0))}) end}
    bag('NumberSequenceKeypoint', {'Time', 'Value', 'Envelope'})
    rt.NumberSequenceKeypoint = {new = function(t, v, e) return newobj('NumberSequenceKeypoint', {Time = n(t, 0), Value = n(v, 0), Envelope = n(e, 0)}) end}
    bag('NumberSequence', {'Keypoints'})
    rt.NumberSequence = {new = function(a, b)
        local kp = {}
        if type(a) == 'table' then kp = a elseif a ~= nil then kp = {rt.NumberSequenceKeypoint.new(0, a), rt.NumberSequenceKeypoint.new(1, b or a)} end
        return newobj('NumberSequence', {Keypoints = kp})
    end}
    bag('ColorSequenceKeypoint', {'Time', 'Value'})
    rt.ColorSequenceKeypoint = {new = function(t, v) return newobj('ColorSequenceKeypoint', {Time = n(t, 0), Value = v}) end}
    bag('ColorSequence', {'Keypoints'})
    rt.ColorSequence = {new = function(a, b)
        local kp = {}
        if type(a) == 'table' then kp = a elseif a ~= nil then kp = {rt.ColorSequenceKeypoint.new(0, a), rt.ColorSequenceKeypoint.new(1, b or a)} end
        return newobj('ColorSequence', {Keypoints = kp})
    end}
    bag('Rect', {'Min', 'Max'})
    rt.Rect = {new = function(a, b, c, d)
        if KIND[a] == 'Vector2' then return newobj('Rect', {Min = a, Max = b, Width = DATA[b].X - DATA[a].X, Height = DATA[b].Y - DATA[a].Y}) end
        return newobj('Rect', {Min = V2(n(a, 0), n(b, 0)), Max = V2(n(c, 0), n(d, 0)), Width = n(c, 0) - n(a, 0), Height = n(d, 0) - n(b, 0)})
    end}
    bag('Ray', {'Origin', 'Direction'})
    rt.Ray = {new = function(o, d) return newobj('Ray', {Origin = o, Direction = d, Unit = nil}) end}
    bag('Region3', {'CFrame', 'Size'})
    rt.Region3 = {new = function(a, b)
        local p, q = DATA[a], DATA[b]
        return newobj('Region3', {CFrame = CF((p.X + q.X) / 2, (p.Y + q.Y) / 2, (p.Z + q.Z) / 2), Size = V3(q.X - p.X, q.Y - p.Y, q.Z - p.Z)})
    end}
    bag('PhysicalProperties', {'Density', 'Friction', 'Elasticity', 'FrictionWeight', 'ElasticityWeight'})
    rt.PhysicalProperties = {new = function(d, f, e, fw, ew) return newobj('PhysicalProperties', {Density = n(d, 0.7), Friction = n(f, 0.3), Elasticity = n(e, 0.5), FrictionWeight = n(fw, 1), ElasticityWeight = n(ew, 1)}) end}
    bag('Font', {'Family', 'Weight', 'Style'})
    rt.Font = {new = function(family, weight, style) return newobj('Font', {Family = family, Weight = weight, Style = style}) end,
        fromEnum = function(e) return newobj('Font', {Family = tostring(e), Weight = rt.enum_item('FontWeight', 'Regular'), Style = rt.enum_item('FontStyle', 'Normal')}) end,
        fromName = function(name) return newobj('Font', {Family = 'rbxasset://fonts/families/' .. tostring(name) .. '.json', Weight = rt.enum_item('FontWeight', 'Regular'), Style = rt.enum_item('FontStyle', 'Normal')}) end}
    local function params(kind, defaults)
        return {new = function() local d = {__mutable = true}; for k, v in pairs(defaults) do d[k] = v end return newobj(kind, d) end}
    end
    bag('RaycastParams', {}); bag('OverlapParams', {}); bag('CatalogSearchParams', {})
    rt.RaycastParams = params('RaycastParams', {FilterDescendantsInstances = {}, FilterType = rt.enum_item('RaycastFilterType', 'Exclude'), IgnoreWater = false, CollisionGroup = 'Default', RespectCanCollide = false})
    rt.OverlapParams = params('OverlapParams', {FilterDescendantsInstances = {}, FilterType = rt.enum_item('RaycastFilterType', 'Exclude'), MaxParts = 0, CollisionGroup = 'Default', RespectCanCollide = false})
    rt.CatalogSearchParams = params('CatalogSearchParams', {SearchKeyword = '', MinPrice = 0, MaxPrice = 0})
    rt.REPR.RaycastParams = function() return 'RaycastParams.new()' end
    rt.REPR.OverlapParams = function() return 'OverlapParams.new()' end
    rt.REPR.CatalogSearchParams = function() return 'CatalogSearchParams.new()' end
end
