-- part 9: start the script, run the scheduler until nothing is left (or a limit is reached), write the end of the log.
do
    local emit = rt.emit
    local CLICK = {MouseButton1Click = true, MouseButton1Down = true, MouseButton1Up = true, MouseButton2Click = true, Activated = true,
        TouchTap = true, Triggered = true, MouseClick = true}

    local function signal_name(sig)
        local k = KIND[sig]
        if k == 'RBXScriptSignal' then return DATA[sig].name end
        if k == 'Ghost' then return tostring(DATA[sig].key or '?') end
        return nil
    end

    -- optional: press every button the script made, one after another, and write down what happens
    function rt.explore()
        local list, seen = {}, {}
        for _, h in ipairs(rt.handlers) do
            local name = h.sig ~= nil and signal_name(h.sig)
            if name and CLICK[name] and not seen[h.fn] then
                seen[h.fn] = true
                list[#list + 1] = {sig = h.sig, fn = h.fn, name = name}
            end
        end
        local per = CONFIG.explore_time or 2
        local maxh = CONFIG.explore_max or 60
        emit('note', 'explore: ' .. #list .. ' click handlers were connected; calling up to ' .. maxh .. ' of them one by one')
        for i = 1, math.min(#list, maxh) do
            local h = list[i]
            emit('note', 'explore: handler ' .. i .. ' of ' .. #list .. ' (' .. h.name .. ')')
            rt.spawn(h.fn)
            rt.run_loop(rt.clock() + per)
        end
        rt.run_loop(rt.clock() + (CONFIG.explore_after or 5))
    end

    local function main()
        local src = CONFIG.source or ''
        local fn, err = loadstring(src, '=' .. (CONFIG.chunkname or 'script'))
        if not fn then
            emit('err', 'syntax error: ' .. tostring(err))
            rt.finish_log('the script has a syntax error', 'steps=0')
            return
        end
        setfenv(fn, rt.ENV)
        local co = cocreate(fn)
        rt.main_thread = co
        rt.resume(co, nil)
        local reason = rt.run_loop()
        if CONFIG.explore then
            rt.explore()
            reason = reason .. '; then the click handlers were called'
        end
        rt.finish_log(reason, 'steps=' .. rt.steps() .. ' time=' .. sfmt('%.3f', rt.clock()))
    end

    local ok, err = xpcall(main, function(e) return debug.traceback(tostring(e), 2) end)
    if not ok then
        pcall(rt.finish_log, 'the sandbox itself failed', tostring(err))
    end
end
