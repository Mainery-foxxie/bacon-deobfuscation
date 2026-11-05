"""The paste host's loader ("Secure Runtime Envelope") as the tests use it.

The text is the loader the host sends for a raw link; the id and the signed token are made up (a real token holds the
address of whoever asked, so none is kept here)."""

PAGE = 'https://paste.example/raw/AbC123xyz'
LINK = 'https://paste.example/api/stream/AbC123xyz?k=eyJmYWtlIjp0cnVlfQ.ZmFrZXNpZw_-'
OTHER_LINK = 'https://paste.example/api/stream/Zz9Zz9Zz9?k=eyJmYWtlIjp0cnVlfQ.b3RoZXI'

REAL_SCRIPT = b'print("the real script")\nlocal x = 1\n'

_TEMPLATE = '''--[=[
    [Bacon Guard Secure Runtime Envelope]
    Build: Protected Runtime
    Notice: Direct raw access is prohibited. Execute via loadstring in Roblox.
]=]
return (function()
    local p = pcall
    local isRoblox = p(function() return game:GetService("Players") end)
    if not isRoblox then
        return print("[Bacon Guard] Direct access prohibited. Live Roblox runtime required.")
    end

    local url = "@LINK@"
    local ok, src = p(function()
        return game:HttpGet(url)
    end)

    if ok and src and #src > 10 then
        local fn, err = loadstring(src)
        if fn then
            return fn()
        else
            error("[Bacon Guard] Execution error: " .. tostring(err))
        end
    else
        warn("[Bacon Guard] Handshake failed or token expired. Please re-run loadstring.")
    end
end)()
'''


def loader(link=LINK):
    """the loader text (str) that points at `link`"""
    return _TEMPLATE.replace('@LINK@', link)
