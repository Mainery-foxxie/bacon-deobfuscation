local net  = require("@lune/net")
local task = require("@lune/task")

local M = {}

M.user_agent    = "bgdeobf-bot/1.0 (script fetch)"
M.roblox_agent  = "Roblox/WinInet"
M.max_bytes     = 8 * 1024 * 1024
M.timeout       = 25
M.max_redirects = 3
M.ports         = { [80] = true, [443] = true }

M.proxy    = "http://CnUvt9XpMADgyJq:IEm97oGXASdTlQv@156.232.90.75:44282"
M.resolver = nil

local function fail(code, msg) error({ code = code, message = msg }, 0) end

local function url_split(u)
    if type(u) ~= "string" then return nil end
    local scheme, rest = u:match("^(%a[%w+.-]*)://(.*)$")
    if not scheme then return nil end
    scheme = scheme:lower()
    local authority, tail = rest:match("^([^/?#]*)(.*)$")
    local userinfo, hostport
    local at = authority:match(".*()@")
    if at then
        userinfo = authority:sub(1, at - 1)
        hostport = authority:sub(at + 1)
    else
        hostport = authority
    end
    local host, port
    if hostport:sub(1, 1) == "[" then
        host, port = hostport:match("^%[([^%]]+)%]:?(%d*)$")
        if not host then return nil end
    else
        host, port = hostport:match("^([^:]*):?(%d*)$")
        if not host then return nil end
    end
    if port == "" then port = nil end
    local path, query, fragment = tail, nil, nil
    local h = path:find("#", 1, true)
    if h then fragment = path:sub(h + 1); path = path:sub(1, h - 1) end
    local q = path:find("?", 1, true)
    if q then query = path:sub(q + 1); path = path:sub(1, q - 1) end
    if path == "" then path = "/" end
    return {
        scheme    = scheme,
        userinfo  = userinfo,
        authority = hostport,
        host      = host and host:lower():gsub("%.$", "") or nil,
        port      = port and tonumber(port) or nil,
        path      = path,
        query     = query,
        fragment  = fragment,
    }
end

local function parse_ipv4(s)
    local a, b, c, d = s:match("^(%d+)%.(%d+)%.(%d+)%.(%d+)$")
    if not a then return nil end
    a, b, c, d = tonumber(a), tonumber(b), tonumber(c), tonumber(d)
    if a > 255 or b > 255 or c > 255 or d > 255 then return nil end
    return { a, b, c, d }
end

local function parse_ipv6(s)
    s = s:gsub("%%.*$", "")
    if not s:find(":", 1, true) then return nil end
    local dc = s:find("::", 1, true)
    local left, right
    if dc then
        left  = s:sub(1, dc - 1)
        right = s:sub(dc + 2)
    else
        left, right = s, ""
    end
    local function groups(part)
        local out = {}
        if part == "" then return out end
        for g in part:gmatch("[^:]+") do
            if g:find(".", 1, true) then
                local v4 = parse_ipv4(g)
                if not v4 then return nil end
                out[#out + 1] = v4[1] * 256 + v4[2]
                out[#out + 1] = v4[3] * 256 + v4[4]
            else
                local n = tonumber(g, 16)
                if not n or n > 0xffff then return nil end
                out[#out + 1] = n
            end
        end
        return out
    end
    local l, r = groups(left), groups(right)
    if not l or not r then return nil end
    local words = {}
    if dc then
        if #l + #r > 7 then return nil end
        for i = 1, #l do words[#words + 1] = l[i] end
        for _ = 1, 8 - #l - #r do words[#words + 1] = 0 end
        for i = 1, #r do words[#words + 1] = r[i] end
    else
        if #l ~= 8 then return nil end
        for i = 1, #l do words[#words + 1] = l[i] end
    end
    local bytes = {}
    for i = 1, #words do
        local w = words[i]
        bytes[#bytes + 1] = math.floor(w / 256)
        bytes[#bytes + 1] = w % 256
    end
    return bytes
end

local function parse_cidr(s)
    local ip, p = s:match("^(.-)/(%d+)$")
    if not ip then return nil end
    local b = parse_ipv4(ip) or parse_ipv6(ip)
    if not b then return nil end
    return { bytes = b, prefix = tonumber(p) }
end

local function in_net(addr, netw)
    local n = netw.prefix
    local full, rem = math.floor(n / 8), n % 8
    for i = 1, full do
        if addr[i] ~= netw.bytes[i] then return false end
    end
    if rem > 0 then
        local mask = bit32.lshift(0xff, 8 - rem)
        if bit32.band(addr[full + 1] or 0, mask) ~= bit32.band(netw.bytes[full + 1] or 0, mask) then
            return false
        end
    end
    return true
end

local BLOCKED4, BLOCKED6 = {}, {}
for _, s in ipairs({
    "0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8",
    "169.254.0.0/16", "172.16.0.0/12", "192.0.0.0/24", "192.0.2.0/24",
    "192.88.99.0/24", "192.168.0.0/16", "198.18.0.0/15", "198.51.100.0/24",
    "203.0.113.0/24", "224.0.0.0/4", "240.0.0.0/4",
}) do BLOCKED4[#BLOCKED4 + 1] = parse_cidr(s) end

for _, s in ipairs({
    "::/128", "::1/128", "64:ff9b::/96", "64:ff9b:1::/48", "100::/64",
    "2001::/23", "2001:db8::/32", "2002::/16", "fc00::/7", "fe80::/10",
    "fec0::/10", "ff00::/8",
}) do BLOCKED6[#BLOCKED6 + 1] = parse_cidr(s) end

local function is_public(bytes)
    if #bytes == 4 then
        for i = 1, #BLOCKED4 do
            if in_net(bytes, BLOCKED4[i]) then return false end
        end
        return true
    end
    local mapped = true
    for i = 1, 10 do
        if bytes[i] ~= 0 then mapped = false; break end
    end
    if mapped and bytes[11] == 0xff and bytes[12] == 0xff then
        return is_public({ bytes[13], bytes[14], bytes[15], bytes[16] })
    end
    for i = 1, #BLOCKED6 do
        if in_net(bytes, BLOCKED6[i]) then return false end
    end
    return true
end

local function check_url(url)
    local sp = url_split(url)
    if not sp then fail("bad_url", "invalid URL") end
    if sp.scheme ~= "http" and sp.scheme ~= "https" then
        fail("bad_url", "only http:// or https:// is allowed")
    end
    if sp.userinfo then fail("bad_url", "the URL must not contain user:password") end
    local host = sp.host
    if not host or host == "" or #host > 253 then fail("bad_url", "invalid URL") end
    if sp.port and not M.ports[sp.port] then
        fail("bad_url", "only port 80 or 443 is allowed")
    end
    local ip = parse_ipv4(host) or parse_ipv6(host)
    if ip and not is_public(ip) then
        fail("blocked", "that address is not a public address")
    end
    return sp
end

local function check_dns(sp)
    if not M.resolver then return end
    if parse_ipv4(sp.host) or parse_ipv6(sp.host) then return end
    local list = M.resolver(sp.host)
    if not list or #list == 0 then fail("net", "cannot resolve " .. sp.host) end
    for i = 1, #list do
        local b = parse_ipv4(list[i]) or parse_ipv6(list[i])
        if not b then fail("net", "resolver returned a bad address") end
        if not is_public(b) then
            fail("blocked", sp.host .. " resolves to a non-public address")
        end
    end
end

local function path_parts(path)
    local out = {}
    for seg in path:gmatch("[^/]+") do out[#out + 1] = seg end
    return out
end

function M.normalize_url(url)
    local sp = url_split(url)
    if not sp then return url end
    local host, parts = sp.host, path_parts(sp.path)
    if (host == "github.com" or host == "www.github.com") and #parts >= 5 and parts[3] == "blob" then
        local rest = {}
        for i = 5, #parts do rest[#rest + 1] = parts[i] end
        return ("https://raw.githubusercontent.com/%s/%s/%s/%s")
            :format(parts[1], parts[2], parts[4], table.concat(rest, "/"))
    end
    if (host == "pastebin.com" or host == "www.pastebin.com") and #parts == 1
        and parts[1]:match("^[A-Za-z0-9]+$") and #parts[1] >= 6 and #parts[1] <= 12 then
        return "https://pastebin.com/raw/" .. parts[1]
    end
    if host == "gist.github.com" and #parts == 2
        and parts[2]:match("^[0-9a-fA-F]+$") and #parts[2] >= 8 and #parts[2] <= 40 then
        return ("https://gist.githubusercontent.com/%s/%s/raw"):format(parts[1], parts[2])
    end
    return url
end

function M.extract_url(text)
    if type(text) ~= "string" then return nil end
    local m = text:match("https?://[^%s<>\"'`%)%]]+")
    if not m then return nil end
    return (m:gsub("[%.,;:!?]+$", ""))
end

function M.url_stem(url)
    local sp = url_split(url)
    local path = sp and sp.path or url
    local seg = path:gsub("/+$", ""):match("([^/]*)$") or ""
    seg = seg:gsub("%.[Ll][Uu][Aa][Uu]?$", ""):gsub("%.[Tt][Xx][Tt]$", "")
    return seg ~= "" and seg or "script"
end

function M.urljoin(base, ref)
    if ref:match("^%a[%w+.-]*://") then return ref end
    local b = url_split(base)
    if not b then return ref end
    if ref:sub(1, 2) == "//" then return b.scheme .. ":" .. ref end
    if ref:sub(1, 1) == "/" then return ("%s://%s%s"):format(b.scheme, b.authority, ref) end
    if ref:sub(1, 1) == "?" then return ("%s://%s%s?%s"):format(b.scheme, b.authority, b.path, ref:sub(2)) end
    if ref:sub(1, 1) == "#" then return ("%s://%s%s"):format(b.scheme, b.authority, b.path) end
    local dir = b.path:gsub("[^/]*$", "")
    return ("%s://%s%s%s"):format(b.scheme, b.authority, dir, ref)
end

local HEX = "0123456789abcdef"
local function random_hex(n)
    local out = {}
    for i = 1, n do
        local k = math.random(1, 16)
        out[i] = HEX:sub(k, k)
    end
    return table.concat(out)
end

local function build_headers(url, user_agent)
    local h = {
        ["User-Agent"]      = user_agent or M.user_agent,
        ["Accept"]          = "text/plain, */*;q=0.5",
        ["Accept-Encoding"] = "gzip, identity",
    }
    if url:match("^https://[%w_%-]+%.roblox%.com/") or url:match("^https://roblox%.com/") then
        h["User-Agent"]       = M.roblox_agent
        h["traceparent"]      = "00-" .. random_hex(49) .. "-00"
        h["Roblox-Id"]        = random_hex(16)
        h["Krnl-Fingerprint"] = random_hex(16)
    end
    return h
end

local function get_header(headers, name)
    if not headers then return nil end
    local n = name:lower()
    for k, v in pairs(headers) do
        if type(k) == "string" and k:lower() == n then
            if type(v) == "table" then return v[1] end
            return v
        end
    end
    return nil
end

local function request_with_timeout(url, user_agent, seconds)
    local state = { done = false, ok = false, res = nil, err = nil }
    task.spawn(function()
        local ok, res = pcall(function()
            return net.request({
                url     = url,
                method  = "GET",
                headers = build_headers(url, user_agent),
            })
        end)
        state.done = true
        state.ok   = ok
        state.res  = res
        if not ok then state.err = res end
    end)
    local ticks = math.max(1, math.floor(seconds / 0.05))
    for _ = 1, ticks do
        if state.done then break end
        task.wait(0.05)
    end
    if not state.done then fail("timeout", "the server took too long to respond") end
    if not state.ok then fail("net", tostring(state.err)) end
    return state.res
end

local function fetch_url(url, opts)
    opts = opts or {}
    local max_bytes     = opts.max_bytes     or M.max_bytes
    local timeout       = opts.timeout       or M.timeout
    local max_redirects = opts.max_redirects or M.max_redirects
    local user_agent    = opts.user_agent

    local cur = url
    for _ = 0, max_redirects do
        local sp = check_url(cur)
        check_dns(sp)
        local resp = request_with_timeout(cur, user_agent, timeout)
        local status = resp.statusCode or 0
        if status >= 300 and status <= 399 then
            local loc = get_header(resp.headers, "location")
            if not loc then fail("http", "redirect without a destination") end
            cur = M.urljoin(cur, loc)
        else
            if status ~= 200 then
                fail("http", ("the server answered HTTP %d"):format(status))
            end
            local body = resp.body or ""
            local cl = get_header(resp.headers, "content-length")
            if cl then
                local n = tonumber(cl)
                if n and n > max_bytes then fail("too_big", "the file exceeds the limit") end
            end
            if #body > max_bytes then fail("too_big", "the file exceeds the limit") end
            if not body:find("%S") then fail("empty", "the content is empty") end
            local ctype = (get_header(resp.headers, "content-type") or ""):lower()
            local head  = body:sub(1, 300):gsub("^%s+", "")
            local lower = head:lower()
            if head:sub(1, 1) == "<"
               and (ctype:find("html", 1, true)
                    or lower:sub(1, 15) == "<!doctype html"
                    or lower:sub(1, 5) == "<html") then
                fail("html", "that is a web page, not a raw file")
            end
            return body
        end
    end
    fail("http", "too many redirects")
end

local function find_loader_link(body, base)
    if #body > 16384 then return nil end
    local patterns = {
        "game:HttpGet%s*%(?%s*[\"']([^\"']+)[\"']",
        "game:HttpGetAsync%s*%(?%s*[\"']([^\"']+)[\"']",
        "[Ll]oadstring%s*%(%s*game:HttpGet%s*%(?%s*[\"']([^\"']+)[\"']",
    }
    for i = 1, #patterns do
        local u = body:match(patterns[i])
        if u and u:match("^https?://") then return M.urljoin(base, u) end
    end
    for u in body:gmatch("[\"'](https?://[^\"']+)[\"']") do
        if u:match("%.lua") or u:match("%.luau") or u:match("%.txt") or u:match("/raw/") then
            return M.urljoin(base, u)
        end
    end
    return nil
end

function M.get(url, opts)
    opts = opts or {}
    url = M.normalize_url(url)
    local body = fetch_url(url, opts)

    if opts.detect_loader == false then
        return body
    end

    local link = find_loader_link(body, url)
    if not link then return body end

    local ok, inner = pcall(fetch_url, link, opts)
    if not ok then
        local e = inner
        if type(e) == "table" and (e.code == "too_big" or e.code == "loader") then error(e, 0) end
        fail("loader", (type(e) == "table" and e.message) or tostring(e))
    end
    return inner
end

M.get_text = M.get

return M
