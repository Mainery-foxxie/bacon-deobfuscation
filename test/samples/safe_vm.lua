-- Self-authored Bacon Guard-style format fixture, not proprietary obfuscator output.
-- Payload: local value = tonumber("41"); print("safe fixture", value); return value
local function module(host, env)
    local xorKey = 37
    local simpleSet = {[10] = true, [11] = true, [12] = true}
    local handlers = {
        [10] = function(r, k, imp, e, u, a, b, c, rf, hf, aux) r[a] = imp(aux) end,
        [11] = function(r, k, imp, e, u, a, b, c, rf, hf, aux) r[a] = k(rf) end,
        [12] = function(r, k, imp, e, u, a, b, c, rf, hf, aux) r[a] = r[b] end,
    }
    local function simple(r, k, imp, e, u, op, a, b, c, rf, hf, aux)
        handlers[op](r, k, imp, e, u, a, b, c, rf, hf, aux)
    end
    local d1 = {}
    local d2 = {}
    local d3 = {}
    local d4 = {}
    local d5 = {}
    local d6 = {}
    local patterns = {[99] = string.char(bit32.bxor(10, xorKey), bit32.bxor(11, xorKey))}
    local patterns2 = {}
    local function mix(a, b, c, rf, hf, aux) return 0 end
    local profiles = {[0] = {}}
    local noVarargs = {}
    local function make(proto, up)
        local recycle
        return function(...)
            local regs = recycle or table.create(proto.maxStack)
            local reader = proto.instructionReader
            local kget = proto.constantReader
            local simpleFields = proto.simpleFields
            local activeSimpleSet = simpleSet
            if simpleFields == nil then
                simpleFields = function(t) return t[1], t[3], t[4], t[5], t[6], t[7], t[8] end
            end
            local imp
            imp = proto.importReader
            local fused
            fused = proto.fusedFields
            if fused == nil then
                fused = function(t) return t[3], t[4], t[5], t[6], t[7], t[8] end
            end
            local capture
            capture = proto.captureFields
            if capture == nil then
                capture = function(t) return t[3], t[4], t[5], t[6] end
            end
            local numParams = proto.numParams
            local varargs = noVarargs
            local nvar = 0
            local top = -1
            local pc = proto.entry
            local fetch = proto.fetchInstruction
            local control
            control = function(op, r, k, imports, e, u, a, b, c, rf, hf, aux, nxt, stackTop)
                if op == 20 then
                    local last = b == 0 and stackTop or a + b - 1
                    local values = table.pack(r[a](table.unpack(r, a + 1, last)))
                    if c == 0 then
                        for i = 1, values.n do r[a + i - 1] = values[i] end
                        stackTop = a + values.n - 1
                    else
                        for i = 1, c - 1 do r[a + i - 1] = values[i] end
                    end
                    return nxt, stackTop
                end
            end
            local profile = profiles[proto.index]
            while true do
                local rec, clear = fetch(pc)
                local op, ef, a, b, c, rf, hf, aux, nxt, alt
                op, ef, a, b, c, rf, hf, aux, nxt, alt = rec[1], rec[2], rec[3], rec[4], rec[5], rec[6], rec[7], rec[8], rec[9], rec[10]
                local sem = rec[2]
                local f1 = d1[sem]
                local f2 = d2[sem]
                local f3 = d3[sem]
                local f4 = d4[sem]
                local f5 = d5[sem]
                if f5 == nil then f5 = d6[sem + proto.index * 256] end
                local pattern = patterns[sem]
                if pattern == nil then pattern = patterns2[sem + proto.index * 256] end
                if f1 ~= nil then
                    return f1(rec, regs, kget, imp, env, up)
                elseif f2 ~= nil then
                    f2(rec, regs, kget, imp, env, up)
                elseif f3 ~= nil then
                    f3(rec, regs, kget, imp, env, up)
                elseif f4 ~= nil then
                    f4(rec, regs, kget, imp, env, up)
                elseif f5 ~= nil then
                    f5(rec, regs, kget, imp, env, up)
                elseif pattern ~= nil then
                    for i = 1, #pattern do
                        local code = bit32.bxor(string.byte(pattern, i), xorKey)
                        local pa, pb, pc_, prf, phf, paux = fused(rec[10 + i])
                        simple(regs, kget, imp, env, up, code, pa, pb, pc_, prf, phf, paux)
                    end
                elseif op == 255 then
                    nxt = bit32.bxor(nxt, mix(a, b, c, rf, hf, aux))
                elseif op == 30 or op == 31 then
                    if op == 30 then
                        if clear then table.clear(rec) end
                        return table.unpack(regs, a, b == 0 and top or a + b - 2)
                    elseif op == 31 then
                        regs[a] = rf
                    end
                elseif activeSimpleSet[op] == true then
                    simple(regs, kget, imp, env, up, op, a, b, c, rf, hf, aux)
                else
                    nxt, top = control(op, regs, kget, imp, env, up, a, b, c, rf, hf, aux, nxt, top)
                end
                pc = nxt
            end
        end
    end
    -- Record fields: opcode, semantic key, A, B, C, RF, HF, AUX, next, alternate.
    -- Import AUX packs the path length in bits 30..31 and its constant index in 20..29.
    local constants = {[0] = "tonumber", [1] = "41", [2] = "print", [3] = "safe fixture"}
    local records = {
        [1] = {0, 99, 0, 0, 0, 0, 0, 0, 2, 4294967295,
            {10, 0, 0, 0, 0, 0, 0, 1073741824, 0, 0},
            {11, 0, 1, 0, 0, 1, 0, 0, 0, 0}},
        [2] = {20, 0, 0, 2, 2, 0, 0, 0, 3, 4294967295},
        [3] = {10, 0, 1, 0, 0, 0, 0, 1075838976, 4, 4294967295},
        [4] = {11, 0, 2, 0, 0, 3, 0, 0, 5, 4294967295},
        [5] = {12, 0, 3, 0, 0, 0, 0, 0, 6, 4294967295},
        [6] = {20, 0, 1, 3, 1, 0, 0, 0, 7, 4294967295},
        [7] = {30, 0, 0, 2, 0, 0, 0, 0, 4294967295, 4294967295},
    }
    local proto = {
        index = 0, numParams = 0, isVararg = true, numUpvalues = 0,
        maxStack = 4, entry = 1, dispatchFormat = 0, vmFamily = 0,
        instructionCount = 7, children = {}, constantData = constants, recordData = records,
        constantReader = function(id) return constants[id] end,
        instructionReader = function(id) return records[id] end,
        fetchInstruction = function(id) return records[id], false end,
        importReader = function(aux) return env[constants[bit32.band(bit32.rshift(aux, 20), 1023)]] end,
    }
    local protos = {[0] = proto}
    local mainIndex = 0
    local main = protos[mainIndex]
    return make(main, {})
end
return module({}, getfenv())()
