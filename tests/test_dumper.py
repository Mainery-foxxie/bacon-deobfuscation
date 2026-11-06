"""The dump script (bgdeobf/dumper.lua.tpl), run in the sandbox against a stand-in of the protector's tables."""
from bgdeobf.patch import _tpl
from bgdeobf.sandbox import run_luau

STAND_IN = '''
local up   -- what a native handler finds when it is run on its own: no upvalues at all
local function proto(aot)
  return {numParams = 0, isVararg = false, numUpvalues = 0, maxStack = 1, entry = 0, dispatchFormat = 0, vmFamily = 0,
          instructionCount = 0, children = {}, constantData = {}, recordData = {}, aotHandler = aot}
end
__RL.PROTOS = {[0] = proto(nil), [1] = proto(function() return up[0] end)}
__RL.MAINIDX = 0
__RL.XORKEY = 0
__RL.MK = function(P, upvalues)
  return function(...)
    -- like the real frame: a native handler, when the proto has one, runs before the point where the dump returns
    if P.aotHandler ~= nil then return P.aotHandler(...) end
    if (...) == __DUMP then return end
    error("the program ran")
  end
end
for ti = 1, 6 do __RL["D" .. ti] = {} end
__RL.P1, __RL.P2, __RL.SIMPLESET = {}, {}, {}
__DUMPER()
print("handler kept: " .. tostring(__RL.PROTOS[1].aotHandler ~= nil))
'''


def dump():
    script = _tpl().replace('%PERM%', '{}').replace('%SEMIDX%', '1') + '\n' + STAND_IN
    return run_luau(script, timeout=20, mem_mb=256).decode('latin1')


def test_a_proto_with_a_native_handler_is_dumped_without_running_the_handler():
    """Newer builds compile some protos to a Lua function (aotHandler) that the frame runs before the dump could return;
    run on its own it indexes the upvalues it was never given.  The dump has to read such a proto like any other."""
    out = dump()
    assert '\nP 0 ' in '\n' + out and '\nP 1 ' in '\n' + out
    assert 'handler kept: true' in out
