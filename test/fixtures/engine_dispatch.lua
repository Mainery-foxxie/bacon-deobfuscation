local function f(a, b, c)
    local ready, flag4, flag5, value = false, false, false, nil
    local state = bit32.bxor(9268137, 1)
    while state do
        if state == 9268136 then
            print("first")
            if math.floor(math.abs(os.clock()) * 10) % 1 == 0 then print("condition kept") end
            state = 5914102
        elseif state == 4610615 then
            flag5 = b
            state = 3620163
        elseif state == 4555853 then
            ready = true
            state = nil
        elseif state == 5914102 then
            flag4 = a
            state = 4610615
        elseif state == 3620163 then
            value = c
            state = 4555853
        end
    end
    return ready, flag4, flag5, value
end
print(f(true, false, "last"))
