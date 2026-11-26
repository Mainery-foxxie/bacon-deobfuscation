return (function()
    local input = buffer.fromstring("\001\001")
    local output = buffer.create(2)
    for offset = 0, 1 do
        buffer.writeu8(output, offset, bit32.bxor(buffer.readu8(input, offset), 73, offset % 256))
    end
    return buffer.tostring(output)
end)()
