"""How the loader of a protected script says in which order the fields of an instruction record are stored.

Older builds pick one of several layouts per function (`if M.dispatchFormat == k then ... elseif ...`).  Newer builds
(v0.43, build d0fca08c7b7b) have no ladder: one layout is spelled out and every function uses it.  These tests run on
small made-up snippets, so they need no protected script."""
import pytest

from bgdeobf.astutil import parse_ast
from bgdeobf.loader import FIXED_FORMATS, LayoutError, Loader


def first_statement(code):
    return parse_ast(code)['body'][0]


def layouts(code):
    # dispatch_perm only reads the statement it is given
    return Loader.__new__(Loader).dispatch_perm(first_statement(code), 10, False)


def test_a_ladder_gives_one_layout_per_format():
    got = layouts('if M.dispatchFormat == 0 then return T[1], T[2], T[3] '
                  'elseif M.dispatchFormat == 3 then return T[3], T[1], T[2] else error("BG:s", 0) end')
    assert got == {0: [1, 2, 3], 3: [3, 1, 2]}


def test_a_ladder_that_assigns_works_too():
    got = layouts('if M.dispatchFormat == 1 then a, b = T[2], T[1] elseif M.dispatchFormat == 0 then a, b = T[1], T[2] end')
    assert got == {1: [2, 1], 0: [1, 2]}


@pytest.mark.parametrize('code', [
    'return T[9], T[4], T[6]',
    'return {T[9], T[4], T[6]}',
    'a, b, c = T[9], T[4], T[6]',
], ids=['return', 'return a table', 'assignment'])
def test_a_layout_without_a_ladder_is_used_by_every_format(code):
    got = layouts(code)
    assert sorted(got) == list(FIXED_FORMATS) == [0, 1, 2, 3]
    assert all(v == [9, 4, 6] for v in got.values())


def test_the_layouts_of_the_formats_do_not_share_one_list():
    got = layouts('return T[1], T[2]')
    got[0].append(99)
    assert got[1] == [1, 2]


@pytest.mark.parametrize('code,message', [
    ('print(1)', 'dispatchFormat body'),
    ('return T[1], 5', 'dispatchFormat index'),
    ('return T[x], T[2]', 'dispatchFormat index'),
    ('if M.size == 0 then return T[1] end', 'dispatchFormat ladder expected'),
])
def test_anything_else_is_refused(code, message):
    with pytest.raises(LayoutError) as e:
        layouts(code)
    assert message in str(e.value)
