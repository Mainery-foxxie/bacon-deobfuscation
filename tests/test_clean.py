"""Tests for the source-level cleanup (bgdeobf/clean.py).  They run the Luau tools, like the engine does."""
import os

import pytest

from bgdeobf.astutil import find_bin, LuauMissing
from bgdeobf.clean import clean_source, tidy, plain_statement
from bgdeobf.engine import compile_check

try:
    find_bin('luau')
    find_bin('luau-ast')
    HAVE_LUAU = True
except LuauMissing:
    HAVE_LUAU = False

pytestmark = pytest.mark.skipif(not HAVE_LUAU, reason='the Luau tools are not installed')

DECRYPT = '''(function()
	local fromstring = buffer.fromstring("\\015?5=/1/(s&>p\\002\\017\\022t,+$-")
	local create = buffer.create(20)

	for i = 0, 19 do
		buffer.writeu8(create, i, bit32.bxor(buffer.readu8(fromstring, i), 91, i % 256))
	end

	return buffer.tostring(create)
end)()'''

FLATTENED = '''(function()
	local fromstring
	local n
	local create
	local n2 = 5520093

	while n2 do
		if n2 == 7671419 then
			for i = 0, n - 1 do
				buffer.writeu8(create, i, bit32.bxor(buffer.readu8(fromstring, i), 230, i % 256))
			end

			n2 = 7376995
		else
			if n2 == 7376995 then
				return buffer.tostring(create)
			end

			if n2 == 5520093 then
				fromstring = buffer.fromstring("\\181\\132\\150\\140\\146\\151\\167\\180\\167")
				n2 = 6825499
			elseif n2 == 6825499 then
				n = 9
				n2 = 251176
			elseif n2 == 251176 then
				create = buffer.create(n)
				n2 = 7671419
			end
		end
	end
end)()'''

TRUE_CHECK = 'bit32.bor(math.floor(os.clock() * 50), 4294967295) == 4294967295'
FALSE_CHECK = 'bit32.bxor(1337, 7331) + math.floor(os.clock() * 50) < 0'


def cleaned(text):
    out, stats = clean_source(text)
    ok, msg, _ = compile_check(out)
    assert ok, '%s\n%s' % (msg, out)
    return out, stats


def test_a_string_decryptor_becomes_its_string():
    out, stats = cleaned('print(%s)\n' % DECRYPT)
    assert out == 'print("Teleport to UGC game")\n'
    assert stats['strings'] == 1


def test_a_decryptor_written_as_a_state_machine_becomes_its_string():
    out, stats = cleaned('local name = %s\n' % FLATTENED)
    assert out.startswith('local name = "') and out.count('\n') == 1 and 'while' not in out
    assert stats['strings'] == 1


def test_brackets_around_a_decrypted_argument_go_with_it():
    out, _ = cleaned('loadstring(game:HttpGet(((%s))))()\n' % DECRYPT)
    assert out == 'loadstring(game:HttpGet("Teleport to UGC game"))()\n'


def test_a_bit32_puzzle_becomes_its_number():
    out, stats = cleaned('local s = bit32.bxor(bit32.bor(1732885655, 1732885947), bit32.band(1732885655, 1732885947))\n')
    assert out == 'local s = 300\n' and stats['numbers'] == 1


def test_a_negative_result_is_bracketed_where_a_sign_would_change_the_meaning():
    out, _ = cleaned('local a = 2 ^ bit32.bxor(4294967295, 1)\nlocal b = x - bit32.bnot(5) + bit32.bxor(0, 0)\n')
    assert out == 'local a = 2 ^ 4294967294\nlocal b = x - 4294967290 + 0\n'
    # bit32 results are unsigned, so a negative value only comes from the arithmetic around them
    cases = [
        ('local a = x - (bit32.bxor(1, 2) - 10)', 'local a = x - (-7)'),       # `x - -7` would read as a sign, `x --7` as a comment
        ('local b = x ^ (bit32.bxor(1, 2) - 10)', 'local b = x ^ (-7)'),
        ('local c = y .. (bit32.bxor(1, 2) - 10)', 'local c = y .. (-7)'),
        ('local d = (bit32.bxor(1, 2) - 10):rep(2)', 'local d = (-7):rep(2)'),
        ('local e = x * (bit32.bxor(1, 2) - 10)', 'local e = x * (-7)'),
        ('local f = x - -bit32.bxor(3, 1)', 'local f = x - (-2)'),
        ('local g = (bit32.bxor(1, 2) - 10) ^ 2', 'local g = 49'),            # the whole expression is closed, so it is one number
        ('local h = -(bit32.bxor(1, 2) - 10)', 'local h = 7'),
        ('local i = {(bit32.bxor(1, 2) - 10), x}', 'local i = {-7, x}'),       # no sign problem next to a comma
        ('foo(bit32.bxor(1, 2) - 10)', 'foo(-7)'),
        ('local j = -bit32.bxor(3, 1) .. "x"', 'local j = "-2x"'),             # a number joined to a string is a string
    ]
    for src, want in cases:
        out, _ = cleaned(src + '\n')
        assert out == want + '\n', (src, out)


def test_a_number_next_to_a_concat_is_bracketed_and_a_string_receiver_too():
    # `x ..3..y` would not lex (`3..` is a malformed number), so a number inside a concat gets brackets
    out, _ = cleaned('print(x .. bit32.bxor(1, 2) .. y)\nprint(x..bit32.bxor(1,2)..y)\nlocal s = (%s):rep(2)\nfoo(bit32.bxor(3, 4))\n' % DECRYPT)
    assert out.splitlines()[0] == 'print(x .. (3) .. y)'
    assert out.splitlines()[1] == 'print(x..(3)..y)'
    assert 'local s = ("Teleport to UGC game"):rep(2)' not in out        # the receiver is joined to the string by the fold ...
    assert 'local s = "Teleport to UGC gameTeleport to UGC game"' in out  # ... and `:rep(2)` is run too, since all of it is closed
    assert 'foo(7)' in out


def test_a_string_literal_that_is_a_method_receiver_is_bracketed():
    out, _ = cleaned('local n = (%s):len() + x\nlocal m = (%s):upper() .. x\n' % (DECRYPT, DECRYPT))
    assert out == 'local n = 20 + x\nlocal m = "TELEPORT TO UGC GAME" .. x\n'
    out, _ = cleaned('print((%s):sub(1, 3) .. x)\nlocal t = x:find((%s))\n' % (DECRYPT, DECRYPT))
    assert out == 'print("Tel" .. x)\nlocal t = x:find("Teleport to UGC game")\n'


def test_a_number_joined_to_literals_is_one_string():
    out, _ = cleaned('print("a" .. bit32.bxor(1, 2) .. "b")\n')
    assert out == 'print("a3b")\n'


def test_an_always_true_check_with_an_unreachable_arm_is_replaced_by_its_body():
    src = 'if %s then\n\tprint(1)\n\tprint(2)\nelseif %s then\n\terror("unreachable", 0)\nend\nprint(3)\n' % (TRUE_CHECK, FALSE_CHECK)
    out, stats = cleaned(src)
    assert out == 'print(1)\nprint(2)\nprint(3)\n'
    assert stats['checks'] == 1 and stats['arms'] == 1


def test_the_else_arm_of_an_always_true_check_goes():
    src = 'if %s then\n\tx = 1\nelse\n\tlocal items = {}\n\titems[1] = "UNREACHABLE_BRANCH"\n\tif not items then\n\t\terror("dead", 0)\n\tend\nend\n' % TRUE_CHECK
    out, _ = cleaned(src)
    assert out == 'x = 1\n'


def test_an_always_false_check_without_an_else_disappears_and_one_with_an_else_leaves_the_else():
    out, _ = cleaned('before()\nif %s then\n\terror("unreachable", 0)\nend\nafter()\n' % FALSE_CHECK)
    assert out == 'before()\nafter()\n'
    out, _ = cleaned('if %s then\n\terror("unreachable", 0)\nelse\n\tgo()\nend\n' % FALSE_CHECK)
    assert out == 'go()\n'


def test_a_false_arm_in_the_middle_of_a_chain_is_dropped_and_the_real_condition_stays():
    src = 'if a then\n\tone()\nelseif %s then\n\ttwo()\nelseif b then\n\tthree()\nelse\n\tfour()\nend\n' % FALSE_CHECK
    out, _ = cleaned(src)
    assert out == 'if a then\n\tone()\nelseif b then\n\tthree()\nelse\n\tfour()\nend\n'
    src = 'if a then\n\tone()\nelseif %s then\n\ttwo()\nelse\n\tfour()\nend\n' % TRUE_CHECK
    out, _ = cleaned(src)
    assert out == 'if a then\n\tone()\nelse\n\ttwo()\nend\n'


def test_a_condition_that_depends_on_the_clock_is_not_decided():
    src = 'if os.clock() > 5 then\n\tlate()\nend\nif math.floor(os.clock()) % 2 == 0 then\n\teven()\nend\n'
    out, stats = clean_source(src)
    assert out == src and stats['checks'] == 0


def test_a_return_that_comes_out_of_its_check_ends_the_block():
    """`if true then return x end` followed by dead code: `return` has to be the last statement of a block"""
    src = 'local function f()\n\tif %s then\n\t\treturn 1\n\tend\n\treturn nil\nend\nprint(f())\n' % TRUE_CHECK
    out, _ = cleaned(src)
    assert out == 'local function f()\n\treturn 1\nend\n\nprint(f())\n' or out == 'local function f()\n\treturn 1\nend\nprint(f())\n'


def test_a_local_that_would_capture_a_later_name_keeps_its_block():
    src = 'local x = 1\nif %s then\n\tlocal x = 2\n\tprint(x)\nend\nprint(x)\n' % TRUE_CHECK
    out, _ = cleaned(src)
    assert out == 'local x = 1\ndo\n\tlocal x = 2\n\tprint(x)\nend\nprint(x)\n'
    src = 'local x = 1\nif %s then\n\tlocal y = 2\n\tprint(y)\nend\nprint(x)\n' % TRUE_CHECK
    out, _ = cleaned(src)
    assert out == 'local x = 1\nlocal y = 2\nprint(y)\nprint(x)\n'


def test_nested_checks_inside_a_decryptor_and_inside_each_other_are_all_taken_out():
    inner = 'if %s then\n\t\tx = 1\n\telse\n\t\terror("unreachable", 0)\n\tend' % TRUE_CHECK
    src = 'if %s then\n\t%s\nend\n' % (TRUE_CHECK, inner)
    out, _ = cleaned(src)
    assert out == 'x = 1\n'


def test_code_the_script_wrote_itself_is_left_alone():
    src = ('local a = math.pi / 2\nlocal b = tostring(5)\nlocal c = string.format("%d", 5)\nif math.huge > 1 then\n\tgo()\nend\n'
           'local d = math.random(1, 9)\nlocal e = game:GetService("Players")\nprint(bit32.band(flags, 1))\n')
    out, stats = clean_source(src)
    assert out == src
    assert not any(stats.values())


def test_a_function_that_does_something_else_than_build_a_value_is_not_run():
    """a call that is not on the harmless list (here `print` and a global write) stops a function from being run: it
    stays a function with its statements, and only the puzzle that is closed on its own inside it becomes a number"""
    src = 'local v = (function()\n\tprint("hi")\n\treturn bit32.bxor(1, 2)\nend)()\nlocal w = (function()\n\tgot = 1\n\treturn bit32.bxor(1, 2)\nend)()\n'
    out, stats = clean_source(src)
    assert out == src.replace('bit32.bxor(1, 2)', '3')
    assert out.count('(function()') == 2 and 'print("hi")' in out and 'got = 1' in out
    assert stats['strings'] == 0 and stats['values'] == 0 and stats['numbers'] == 2


def test_a_decryptor_that_never_ends_is_left_alone_and_does_not_hang():
    src = 'local v = (function()\n\twhile true do\n\t\tlocal x = bit32.bxor(1, 2)\n\tend\n\treturn 1\nend)()\nprint(1)\n'
    out, _ = clean_source(src, timeout=3, budget=8)
    assert out == src


def test_cleaning_twice_changes_nothing():
    src = 'if %s then\n\tprint(%s)\nelseif %s then\n\terror("unreachable", 0)\nend\n' % (TRUE_CHECK, DECRYPT, FALSE_CHECK)
    once, _ = cleaned(src)
    twice, stats = clean_source(once)
    assert once == twice and not any(stats.values())


def test_tidy_only_removes_the_blank_lines_a_removed_wrapper_leaves():
    assert tidy('a()\n\n\n\nb()\n') == 'a()\nb()\n'                       # plain statements are never apart in the printer's layout
    assert tidy('if x then\n\n\ta()\n\nend\n') == 'if x then\n\ta()\nend\n'
    assert tidy('local function f()\n\n\ta()\nend\n') == 'local function f()\n\ta()\nend\n'
    # blank lines around blocks and after a call that opens a multi-line expression stay
    kept = 'a()\n\nif x then\n\tb()\nend\n\nc()\n'
    assert tidy(kept) == kept
    assert tidy('foo(function() return 1 end)\n\nbar()\n') == 'foo(function() return 1 end)\nbar()\n'
    long = 'x = [[a\n\n\nb]]\n'
    assert tidy(long) == long


def test_plain_statement():
    assert plain_statement('\tfoo(1, "a)")')
    assert not plain_statement('\tif x then')
    assert not plain_statement('\tfoo(function()')
    assert not plain_statement('\tlocal t = {')
    assert not plain_statement('\treturn 1')
