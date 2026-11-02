-- bgdeobf: a fake Roblox environment that runs a script and writes down what it does.  Nothing here touches a network,
-- a file or the clock of the machine: time is simulated, every request is answered by a stub.
-- This text is put behind `local CONFIG = {...}` (written by tracer.py) and runs in the plain Luau command line tool.
-- The runtime keeps real functions in locals and then gives its own code an empty environment, so nothing of it is
-- reachable through getfenv() and a typing mistake in a global name shows up in the tests (see test_tracer.py).
local type, typeof, tostring, tonumber = type, typeof, tostring, tonumber
local pcall, xpcall, error, assert, select, next = pcall, xpcall, error, assert, select, next
local pairs, ipairs, unpack = pairs, ipairs, unpack
local rawget, rawset, rawequal, rawlen = rawget, rawset, rawequal, rawlen
local setmetatable, getmetatable = setmetatable, getmetatable
local newproxy, loadstring, getfenv, setfenv = newproxy, loadstring, getfenv, setfenv
local print, collectgarbage = print, collectgarbage
-- the runtime works on its own copies of the library tables: a script that changes `string` or `table` can not break it
local function clone(t)
    local o = {}
    for k, v in next, t do o[k] = v end
    return o
end
local string, table, math, os, coroutine, debug = clone(string), clone(table), clone(math), clone(os), clone(coroutine), clone(debug)
local utf8, bit32, buffer = clone(utf8), clone(bit32), clone(buffer)
local REAL_G = getfenv(0)
local RUNTIME_ENV = {}
local CHUNK = debug.info(1, 's')    -- the name Luau puts in front of an error that is raised inside the runtime itself
setfenv(1, RUNTIME_ENV)

local rt = {}                       -- what the parts of the runtime share
rt.REAL_G = REAL_G
rt.RUNTIME_ENV = RUNTIME_ENV
rt.CHUNK = CHUNK
rt.real = {                         -- the real functions that scripts get to see unchanged or wrapped
    type = type, typeof = typeof, tostring = tostring, tonumber = tonumber, pcall = pcall, xpcall = xpcall,
    error = error, assert = assert, select = select, next = next, pairs = pairs, ipairs = ipairs, unpack = unpack,
    rawget = rawget, rawset = rawset, rawequal = rawequal, rawlen = rawlen, setmetatable = setmetatable,
    getmetatable = getmetatable, newproxy = newproxy, loadstring = loadstring, getfenv = getfenv, setfenv = setfenv,
    print = print, collectgarbage = collectgarbage, string = string, table = table, math = math, os = os, coroutine = coroutine, debug = debug,
    utf8 = utf8, bit32 = bit32, buffer = buffer,
}
local floor, huge = math.floor, math.huge
local sfmt, ssub, sgsub, sbyte, srep = string.format, string.sub, string.gsub, string.byte, string.rep
local tconcat, tinsert = table.concat, table.insert
local cocreate, coresume, coyield, corunning, costatus = coroutine.create, coroutine.resume, coroutine.yield, coroutine.running, coroutine.status

local function pack(...) return {n = select('#', ...), ...} end
rt.pack = pack
local function weak(mode) return setmetatable({}, {__mode = mode or 'k'}) end
rt.weak = weak

-- every fake object (Instance, Vector3, signal, ...) is a userdata made with newproxy; these two tables hold what it is
-- and its state.  KIND: userdata -> 'Instance' | 'Vector3' | 'RBXScriptSignal' | ...   DATA: userdata -> state table
local KIND, DATA = weak(), weak()
rt.KIND, rt.DATA = KIND, DATA
