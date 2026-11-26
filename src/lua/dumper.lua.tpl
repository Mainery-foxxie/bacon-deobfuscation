__DUMP = {}
__RL = {}
local __hex = function(s)
  local t = {}
  for i = 1, #s do t[i] = string.format("%02x", string.byte(s, i)) end
  return table.concat(t)
end
local __fld = function(v)
  if v == nil then return "-" end
  return tostring(v)
end
local __PERM = %PERM%
local __SEMIDX = %SEMIDX%
function __DUMPER()
  local RL = __RL
  local out = {}
  local function emit(s) out[#out + 1] = s end
  local protos, mk = RL.PROTOS, RL.MK
  local numeric = RL.NUMERIC
  local n = 0
  for k in pairs(protos) do n += 1 end
  emit("N " .. n .. " xorkey " .. tostring(RL.XORKEY) .. " main " .. tostring(RL.MAINIDX))
  for ti = 1, 6 do
    local keys = {}
    for k in pairs(RL["D" .. ti]) do keys[#keys + 1] = k end
    table.sort(keys)
    emit("D " .. ti .. " " .. table.concat(keys, ","))
  end
  do
    local keys = {}
    for k, v in pairs(RL.P1) do keys[#keys + 1] = k end
    table.sort(keys)
    for _, k in ipairs(keys) do emit("P1 " .. k .. " " .. __hex(RL.P1[k])) end
    keys = {}
    for k, v in pairs(RL.P2) do keys[#keys + 1] = k end
    table.sort(keys)
    for _, k in ipairs(keys) do emit("P2 " .. k .. " " .. __hex(RL.P2[k])) end
    keys = {}
    for k in pairs(RL.SIMPLESET) do keys[#keys + 1] = k end
    table.sort(keys)
    emit("S " .. table.concat(keys, ","))
  end
  for idx = 0, n - 1 do
    local key = RL.PROTO_MAP and RL.PROTO_MAP(idx) or idx
    local P = protos[key]
    if P == nil then error("prototype mapping missing", 0) end
    if RL.PROTO_INIT ~= nil then RL.PROTO_INIT(P) end
    local masks = {}
    if numeric ~= nil and numeric.masks ~= nil then
      for slot, key in pairs(numeric.masks[P[numeric.dispatch_format]] or {}) do
        local mask = P[key]
        if type(mask) ~= "number" then error("record mask missing", 0) end
        masks[slot] = mask
      end
    end
    local function field(record, index)
      local value, mask = record[index], masks[index]
      if value == nil then return nil end
      if mask ~= nil then return bit32.bxor(value, mask) end
      return value
    end
    -- newer builds compile some protos to a native Lua function (aotHandler) that the frame runs before the point
    -- where the dump returns; on its own it would index upvalues that do not exist.  The records are all we want, so
    -- the handler is set aside while the frame is made and put back afterwards.
    local aot
    if numeric ~= nil then
      aot = P[numeric.aot_handler]
      P[numeric.aot_handler] = nil
    else
      aot = P.aotHandler
      P.aotHandler = nil
    end
    local f = mk(P, {})
    local instructionReader, constantReader = f(__DUMP)
    if numeric ~= nil then
      P[numeric.aot_handler] = aot
      local raw = P
      P = {
        numParams = raw[numeric.num_params], isVararg = raw[numeric.is_vararg],
        numUpvalues = raw[numeric.num_upvalues], maxStack = raw[numeric.max_stack],
        entry = raw[numeric.entry], dispatchFormat = raw[numeric.dispatch_format],
        vmFamily = raw[numeric.vm_family], regShift = raw[numeric.reg_shift],
        instructionCount = 0, children = raw[numeric.children],
        constantData = raw[numeric.constant_data], recordData = raw[numeric.record_data],
        instructionReader = instructionReader or raw[numeric.record_reader],
        constantReader = constantReader or raw[numeric.constant_reader],
      }
      for _ in pairs(P.recordData) do P.instructionCount += 1 end
    else
      P.aotHandler = aot
    end
    emit(string.format("P %d np=%d va=%d nup=%d ms=%d entry=%d df=%d vf=%d nrec=%d rs=%d", idx, P.numParams, P.isVararg and 1 or 0,
      P.numUpvalues, P.maxStack, P.entry, P.dispatchFormat, P.vmFamily, P.instructionCount, P.regShift or 0))
    local kids = {}
    for i = 0, 1000000 do
      local c = P.children[i]
      if c == nil then break end
      kids[#kids + 1] = c
    end
    emit("H " .. idx .. " " .. table.concat(kids, ","))
    local cids = {}
    for cid in pairs(P.constantData) do cids[#cids + 1] = cid end
    table.sort(cids)
    for _, cid in ipairs(cids) do
      local v = P.constantReader(cid)
      local tv = type(v)
      if tv == "string" then emit(string.format("C %d %d s %s", idx, cid, __hex(v)))
      elseif tv == "number" then emit(string.format("C %d %d n %.17g", idx, cid, v))
      elseif tv == "boolean" then emit(string.format("C %d %d b %d", idx, cid, v and 1 or 0))
      elseif tv == "nil" then emit(string.format("C %d %d z", idx, cid))
      elseif tv == "table" and type(v[2]) == "table" and type(v[2][1]) == "table" then
        local items = {}
        for _, kv in ipairs(v[2]) do items[#items + 1] = kv[1] .. ":" .. kv[2] end
        emit(string.format("C %d %d t %s", idx, cid, table.concat(items, ",")))
      elseif tv == "table" and type(v[2]) == "table" then
        emit(string.format("C %d %d i %s", idx, cid, table.concat(v[2], ",")))
      else error("unsupported constant type: " .. tv, 0) end
    end
    local rids = {}
    for rid in pairs(P.recordData) do rids[#rids + 1] = rid end
    table.sort(rids)
    local df = P.dispatchFormat
    local pm = __PERM[df]
    for _, rid in ipairs(rids) do
      local T = P.instructionReader(rid)
      local raw = {}
      for i = 1, 10 do raw[i] = __fld(field(T, i)) end
      local sem = T[__SEMIDX]
      local parts = {}
      local pi = 11
      while T[pi] ~= nil do
        local pr = {}
        for i = 1, 10 do pr[i] = __fld(field(T[pi], i)) end
        parts[#parts + 1] = table.concat(pr, ",")
        pi += 1
      end
      local dedn = "-"
      if RL.D1[sem] ~= nil then dedn = "1"
      elseif RL.D2[sem] ~= nil then dedn = "2"
      elseif RL.D3[sem] ~= nil then dedn = "3"
      elseif RL.D4[sem] ~= nil then dedn = "4"
      elseif RL.D5[sem] ~= nil then dedn = "5"
      elseif RL.D6[sem + idx * 256] ~= nil then dedn = "6" end
      local pt = RL.P1[sem]
      if pt == nil then pt = RL.P2[sem + idx * 256] end
      local mix = "-"
      if RL.MIX ~= nil and pm ~= nil then
        local ok, r = pcall(RL.MIX, field(T, pm[3]), field(T, pm[4]), field(T, pm[5]), field(T, pm[6]), field(T, pm[7]), field(T, pm[8]))
        if ok and type(r) == "number" then mix = string.format("%d", r) end
      end
      emit(string.format("R %d %d sem=%d ded=%s pat=%s raw=%s parts=%s mix=%s", idx, rid, sem, dedn, pt and __hex(pt) or "-", table.concat(raw, ","), table.concat(parts, ";"), mix))
    end
  end
  print(table.concat(out, "\n"))
end
