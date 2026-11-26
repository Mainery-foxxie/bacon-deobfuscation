local function trap()
    local t = {}
    t[t] = t
    return setmetatable(t, {__index=t, __tostring=function() return "decoy" end})
end
local function decoys() return "one", "two", "three", "four" end
local function install()
    local decoy = trap()
    local target = getgenv()
    local names = {"getconstants", "getprotos", "getupvalues", "getconstant", "getproto", "getupvalue"}
    for _, name in ipairs(names) do
        local original = target[name]
        target[name] = function(fn)
            if fn == install or fn == decoys then return decoy end
            return original(fn)
        end
    end
end
install()
local function verifyEnvironment()
    if type(buffer) ~= "table" or type(bit32) ~= "table" then return false end
    local data = buffer.create(4)
    buffer.writeu8(data, 0, 186)
    if buffer.readu8(data, 0) ~= 186 then return false end
    if debug and debug.info and debug.info(buffer.readu8, "s") ~= "[C]" then return false end
    if string and string.dump and pcall(string.dump, bit32.bxor) then return false end
    return true
end
if not verifyEnvironment() then
    error("environment verification failed", 0)
end
if applicationKey ~= "valid" then error("application key failed", 0) end
print("application reached")
