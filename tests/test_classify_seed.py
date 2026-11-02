"""How the classifier seeds the loader's own top-level tables before it runs a handler.

The classifier runs the loader's handler code against reference operations.  In some builds (found with v0.43, build forge43b)
the handler of DUPTABLE (a table made from a constant template) tells a template from any other constant by comparing the
template's first field with a table the loader makes at its top level, `local _P = {}`.  That table is a tag, not a table of
handlers.  When the classifier filled it as an empty table the comparison could never be true, DUPTABLE was taken for NEWTABLE
and the fields of every constant table were lost without anything failing.  These tests use a made-up handler of that shape,
so they need no protected script."""
from types import SimpleNamespace

from bgdeobf.astutil import parse_ast
from bgdeobf.classify import Classifier
from bgdeobf.luavm import LuaTable
from bgdeobf.refspec import SPECS

ROLES = ('REGS', 'KGET', 'A', 'RF', 'NXT', 'TOP')

HANDLER = '''
local R, KGET, A, RF, NXT, TOP
local _P = {}
local function handler()
    local tpl = KGET(RF)
    local t = {}
    if tpl[1] == _P then
        for i = 1, #tpl[2] do
            local pair = tpl[2][i]
            if pair[2] >= 0 then
                t[KGET(pair[1])] = KGET(pair[2])
            end
        end
    end
    R[A] = t
end
'''


def classifier(items=()):
    """a classifier over a made-up loader: the locals of the first line are the handler's roles, `_P` is its tag"""
    body = parse_ast(HANDLER)['body']
    loader = SimpleNamespace(role_local=dict(zip(ROLES, body[0]['vars'])), junk_op=None, frame_body=body, roles={}, funcs={},
                             simple_table_items=[(body[1]['vars'][0], list(items))])
    return Classifier(loader), body[2]['func']['body'], body[1]['vars'][0]['location']


def state():
    return SimpleNamespace(f=dict(op=0, ef=0, A=1, B=2, C=3, RF=4, HF=0, AUX=0), tag=object(), R=LuaTable(), kget=None, imp=None,
                           fused=None, UP=LuaTable(), nxt=0, alt=0, top=0, rec=LuaTable(), clr=False, prof=LuaTable(), varargs=LuaTable(),
                           nvar=0)


def test_a_table_template_is_recognised_by_the_tag_the_loader_made():
    c, block, _ = classifier()
    run = c.run_block(block)
    assert c.match(run, SPECS['DUPTABLE'])                   # the fields of the template reach the new table
    assert not c.match(run, SPECS['NEWTABLE'])               # ... so this is not an empty table


def test_an_empty_table_is_the_tag_and_stays_unseeded():
    c, _, tag_loc = classifier()
    st = state()
    cells = c.seed(st)
    assert tag_loc not in cells                              # nothing was put there ...
    assert c.interp.unbound(tag_loc) is st.tag               # ... so a handler that reads it gets the tag of the test's templates


def test_a_table_with_values_is_still_seeded():
    c, _, loc = classifier([(7, {'type': 'AstExprConstantNumber', 'value': 10.0}),
                            (8, {'type': 'AstExprConstantString', 'value': 'x'}),
                            (9, {'type': 'AstExprConstantBool', 'value': True})])
    cells = c.seed(state())
    tab, = cells[loc]
    assert isinstance(tab, LuaTable) and (tab.get(7), tab.get(8), tab.get(9)) == (10, 'x', True)
