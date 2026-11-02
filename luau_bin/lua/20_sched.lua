-- part 2: simulated time and the scheduler.  Nothing ever sleeps: when every thread is waiting, the clock jumps to the
-- moment the first one wakes up.  `task.wait(5)` costs no real time, and a `while true do task.wait() end` loop runs
-- until the simulated time limit.
do
    local now = 0
    local H, hn, seq = {}, 0, 0
    local nother = 0                     -- entries in the queue that are not the frame ticker
    local current = nil
    local waiting = weak()               -- thread -> the queue entry that will wake it (to cancel it)
    local MAXT = CONFIG.max_time or 30
    local SETTLE = CONFIG.settle_time or 10
    local MAXSTEPS = CONFIG.max_steps or 200000
    local steps = 0
    local CLOCK0 = CONFIG.clock0 or 37.5

    function rt.clock() return now end
    function rt.steps() return steps end

    -- time seen by scripts: asking for the time moves it a little, so `repeat until os.clock() - t > 1` ends
    local TICK = 0.0001
    function rt.query_clock()
        now = now + TICK
        return now
    end
    function rt.epoch() return (CONFIG.epoch or 1767225600) + now end
    function rt.cpu_clock() return CLOCK0 + now end

    -- ---- the queue: a binary heap ordered by (time, order of insertion)
    local function less(a, b)
        if a.t ~= b.t then return a.t < b.t end
        return a.seq < b.seq
    end
    local function push(e)
        seq = seq + 1
        e.seq = seq
        if e.tag ~= 'frame' then nother = nother + 1 end
        hn = hn + 1
        local i = hn
        H[i] = e
        while i > 1 do
            local p = i // 2
            if less(H[i], H[p]) then
                H[i], H[p] = H[p], H[i]
                i = p
            else
                break
            end
        end
    end
    local function pop()
        if hn == 0 then return nil end
        local top = H[1]
        H[1] = H[hn]
        H[hn] = nil
        hn = hn - 1
        local i = 1
        while true do
            local l, r, m = i * 2, i * 2 + 1, i
            if l <= hn and less(H[l], H[m]) then m = l end
            if r <= hn and less(H[r], H[m]) then m = r end
            if m == i then break end
            H[i], H[m] = H[m], H[i]
            i = m
        end
        if top.tag ~= 'frame' then nother = nother - 1 end
        return top
    end

    -- ---- threads
    -- an error raised inside the runtime (not in the script) starts with the runtime's own chunk name; the script's
    -- chunk is called "script".  Such an error is a limit of the fake environment: the line is dropped and it is marked.
    function rt.internal_error(msg)
        local pre = rt.CHUNK .. ':'
        if type(msg) == 'string' and string.sub(msg, 1, #pre) == pre then
            return (string.gsub(msg, '^[^\n]-:%d+: ', '', 1)), true
        end
        return msg, false
    end

    function rt.thread_error(co, err)
        local msg = err
        if type(err) ~= 'string' then msg = rt.repr(err) end
        local internal
        msg, internal = rt.internal_error(msg)
        local tb, where = '', nil
        local ok, t = pcall(debug.traceback, co)
        if ok and type(t) == 'string' then
            local n = 0
            for ln in string.gmatch(t, '[^\n]+') do
                n = n + 1
                if n <= 6 then tb = tb .. (n > 1 and ' | ' or '') .. ln end
                if not where and string.sub(ln, 1, #rt.CHUNK + 1) ~= rt.CHUNK .. ':' then
                    local src, num = string.match(ln, '^([%w_%.%-]+):(%d+)')
                    if src and src ~= 'stacktrace' then where = src .. ':' .. num end
                end
            end
        end
        -- a library function that was called through one of the runtime's wrappers complains with the runtime's position
        -- in front; to the script it is an ordinary error of its own line
        if internal and where and string.find(msg, "^invalid argument #%d+ to '") then
            msg, internal = where .. ': ' .. msg, false
        end
        local at = where
        if at and rt.MAIN and string.sub(at, 1, #rt.MAIN + 1) == rt.MAIN .. ':' then at = string.sub(at, #rt.MAIN + 2) end
        rt.emit('err', 'error: ' .. msg, tb, internal and 'internal' or nil, at)
    end

    local function resume(co, args)
        local prev = current
        current = co
        local ok, err
        if args then ok, err = coresume(co, unpack(args, 1, args.n)) else ok, err = coresume(co) end
        current = prev
        if not ok then rt.thread_error(co, err) end
        return ok
    end
    rt.resume = resume

    -- run f(...) in a new thread right now, until it waits for the first time (task.spawn)
    function rt.spawn(f, ...)
        local co = type(f) == 'thread' and f or cocreate(f)
        resume(co, pack(...))
        return co
    end

    -- wake a thread later: delay seconds from now
    function rt.schedule(co, delay, args)
        local e = {t = now + (delay > 0 and delay or 0), co = co, args = args, start = now}
        waiting[co] = e
        push(e)
        return e
    end
    -- call a function of the runtime later (it must not yield)
    function rt.later(delay, fn, tag)
        local e = {t = now + (delay > 0 and delay or 0), fn = fn, tag = tag}
        push(e)
        return e
    end
    -- a new thread that starts after delay seconds (task.delay / task.defer)
    function rt.start_later(delay, f, args)
        local co = type(f) == 'thread' and f or cocreate(f)
        rt.schedule(co, delay, args or pack())
        return co
    end

    function rt.cancel(co)
        local e = waiting[co]
        if e then
            e.cancelled = true
            waiting[co] = nil
        end
        if type(co) == 'thread' and costatus(co) == 'suspended' then pcall(coroutine.close, co) end
    end

    -- the current thread sleeps (min: the shortest sleep, like one frame)
    function rt.sleep(dt, min)
        dt = tonumber(dt) or 0
        if dt ~= dt or dt < min then dt = min end
        local co = corunning()
        rt.schedule(co, dt, nil)
        return coyield()
    end

    -- the current thread waits until someone calls rt.wake(thread, ...)
    function rt.park()
        return coyield()
    end
    function rt.wake(co, ...)
        rt.schedule(co, 0, pack(...))
    end
    function rt.current() return current end

    -- ---- frames: Heartbeat, RenderStepped and Stepped fire 60 times a second as long as someone listens
    local ticker = false
    function rt.ensure_ticker()
        if ticker then return end
        ticker = true
        local function tick()
            local dt = 1 / 60
            if rt.fire_frame then rt.fire_frame(dt) end
            rt.later(dt, tick, 'frame')
        end
        rt.later(1 / 60, tick, 'frame')
    end

    -- ---- the main loop; returns why it stopped.  With a deadline it runs only until that simulated time.
    function rt.run_loop(deadline)
        local limit = deadline or MAXT
        while true do
            local top = H[1]
            if top == nil then return 'every thread has finished or is waiting for an event that never comes' end
            if top.t > limit then
                if deadline then now = deadline; return 'deadline' end
                return 'the simulated time limit of ' .. MAXT .. ' s was reached'
            end
            local e = pop()
            if nother == 0 and e.tag == 'frame' and e.t >= SETTLE and not deadline then
                return 'only frame handlers (Heartbeat/RenderStepped) were left after ' .. SETTLE .. ' s'
            end
            if not e.cancelled then
                steps = steps + 1
                if steps > MAXSTEPS then return 'the step limit of ' .. MAXSTEPS .. ' was reached' end
                if e.t > now then now = e.t end
                if e.fn then
                    local ok, err = pcall(e.fn)
                    if not ok then rt.emit('err', 'error in the sandbox: ' .. tostring(err)) end
                else
                    waiting[e.co] = nil
                    if e.args then
                        resume(e.co, e.args)
                    else
                        resume(e.co, pack(now - e.start))
                    end
                end
            end
        end
    end
end
