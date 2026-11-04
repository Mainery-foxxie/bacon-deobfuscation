"""The pieces of the bytecode comparison that decide what counts as the same program."""
import re
import subprocess

import pytest

from bgdeobf.astutil import find_bin, LuauMissing
from bgdeobf.fidelity import ann, only_constant_loads_differ, sstr, unq

try:
    COMPILE = find_bin('luau-compile')
except LuauMissing:
    COMPILE = None


def test_a_string_constant_shows_control_characters_the_way_luau_compile_prints_them():
    assert sstr(b'a\x01\x1fb\n\t') == 'a\\x01\\x1Fb\\x0A\\x09'
    assert sstr(b'plain text') == 'plain text'
    assert sstr('already text') == 'already text'
    # DEL and the bytes from 0x80 up are printed as they are
    assert sstr(b'\x7f\xc8') == '\x7f\xc8'


def test_a_string_constant_is_cut_to_24_characters_of_the_written_out_form():
    assert sstr(b'\x01' * 10) == '\\x01' * 6
    assert sstr(b'x' * 40) == 'x' * 24


@pytest.mark.skipif(COMPILE is None, reason='luau-compile is not installed')
def test_the_constant_from_the_bytecode_and_the_one_luau_compile_prints_are_the_same_text(tmp_path):
    cases = [b'a\x01\x1fb\n\t end of a long string that goes past twenty four chars',
             b'short\x02', b'\x01\x02\x03\x04\x05\x06\x07\x08\x0b\x0c', b'tab\there', b'x' * 60]
    for raw in cases:
        lit = ''.join('\\%d' % c if c < 32 else chr(c) for c in raw)
        src = tmp_path / 'k.lua'
        src.write_bytes(('local s = "%s"\nprint(s)\n' % lit).encode('latin1'))
        text = subprocess.run([COMPILE, '--text', str(src)], capture_output=True, check=True).stdout.decode('latin1')
        shown = [ann(ln) for ln in text.splitlines() if 'LOADK' in ln and ann(ln) and ann(ln).startswith("'")]
        assert len(shown) == 1, text
        assert unq(shown[0]) == sstr(raw), (raw, shown[0])


def test_a_number_loaded_in_another_place_is_the_same_program():
    a = ['CALL:-1:1', 'JEQ', 'K:N0', 'IMP:type', 'CALL:1:1', 'OP:-', 'K:N1', 'FORNPREP']
    b = ['CALL:-1:1', 'JEQ', 'IMP:type', 'CALL:1:1', 'OP:-', 'K:N0', 'K:N1', 'FORNPREP']
    assert only_constant_loads_differ(a, b)
    assert only_constant_loads_differ(b, a)


def test_a_nil_load_may_move_too():
    assert only_constant_loads_differ(['LN', 'K:N1', 'RET:0'], ['K:N1', 'LN', 'RET:0'])
    assert only_constant_loads_differ(['LN', 'K:N1', 'RET:0'], ['K:N1', 'RET:0'])


def test_the_same_tokens_are_not_a_difference():
    a = ['K:N0', 'IMP:type', 'CALL:1:1', 'RET:0']
    assert not only_constant_loads_differ(a, list(a))


def test_a_different_number_or_a_number_loaded_a_different_number_of_times_is_a_difference():
    a = ['K:N0', 'IMP:type', 'CALL:1:1', 'RET:0']
    assert not only_constant_loads_differ(a, ['IMP:type', 'K:N1', 'CALL:1:1', 'RET:0'])
    assert not only_constant_loads_differ(a, ['K:N0', 'K:N0', 'IMP:type', 'CALL:1:1', 'RET:0'])
    assert not only_constant_loads_differ(a, ['IMP:type', 'CALL:1:1', 'RET:0'])


def test_a_string_that_moves_is_a_difference():
    """`print("a"); print("b")` and `print("b"); print("a")` have the same tokens apart from the order of the strings"""
    a = ['IMP:print', 'K:Sa', 'CALL:1:0', 'IMP:print', 'K:Sb', 'CALL:1:0', 'RET:0']
    b = ['IMP:print', 'K:Sb', 'CALL:1:0', 'IMP:print', 'K:Sa', 'CALL:1:0', 'RET:0']
    assert not only_constant_loads_differ(a, b)
    # nor does a string load at the top count as the load of the same string next to its use
    assert not only_constant_loads_differ(['K:Sa', 'IMP:print', 'CALL:1:0', 'RET:0'], ['IMP:print', 'K:Sa', 'CALL:1:0', 'RET:0'])


def test_another_call_or_jump_is_a_difference_whatever_the_constants_do():
    a = ['K:N0', 'IMP:type', 'CALL:1:1', 'RET:0']
    assert not only_constant_loads_differ(a, ['K:N0', 'IMP:typeof', 'CALL:1:1', 'RET:0'])
    assert not only_constant_loads_differ(a, ['IMP:type', 'K:N0', 'CALL:1:1', 'JMP', 'RET:0'])
