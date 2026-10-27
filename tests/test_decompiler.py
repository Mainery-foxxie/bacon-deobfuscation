"""Decompiler tests on small hand-assembled VM programs, so they run without a protected script.

Each program uses the register layout that the Roblox Luau compiler produced in a real script that once exposed a
bug.  The result is rendered, compiled again with luau-compile and compared with the original through the same
bytecode check the engine runs on every job."""
import os
import re
import tempfile

import pytest

from bgdeobf import fidelity
from bgdeobf.decomp import fmt_num
from bgdeobf.engine import compile_check
from bgdeobf.render import render

NONE = 4294967295
COND = ('JUMPIF', 'JUMPIFNOT', 'JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT', 'JUMPIFNOTLE', 'JUMPIFNOTLT')


def I(op, to=None, **fields):
    """one instruction: operands as keywords (A, B, C, rf, aux); `to` is the target index of a jump"""
    return (op, fields, to)


def s(text):
    return ('s', text.encode())


def num(value):
    return ('n', float(value))


def import_aux(*name_ids):
    """GETIMPORT operand: how many names, then the constant ids of the path"""
    aux = len(name_ids) << 30
    for i, k in enumerate(name_ids):
        aux |= k << (20 - 10 * i)
    return aux


class FakeDecoder(object):
    """Stands in for bgdeobf.decode.Decoder: every instruction is one record, in order."""

    def __init__(self, protos):
        self.code = {}
        self.protos = {}
        for pid, spec in protos.items():
            self.code[pid] = spec['code']
            self.protos[pid] = dict(np=spec.get('np', 0), va=0, nup=spec.get('nup', 0), kids=spec.get('kids', []),
                                    consts=spec['consts'], entry=0, recs=dict((i, None) for i in range(len(spec['code']))))

    def decode_all(self, pid):
        return dict((rid, self.decode(pid, rid)) for rid in self.protos[pid]['recs'])

    def decode(self, pid, rid):
        op, fields, to = self.code[pid][rid]
        a = dict(A=0, B=0, C=0, rf=0, Hf=0, aux=0)
        a.update(fields)
        d = dict(kind=op, sem=0, ins=[], ctrl=None, nxt=rid + 1, alt=NONE, rid=rid)
        if op == 'RETURN':
            d['ctrl'], d['nxt'] = (op, a), NONE
        elif op in COND or op in ('JUMP', 'FORNPREP', 'FORNLOOP', 'FORGLOOP'):
            d['ctrl'], d['alt'] = (op, a), to
        elif op == 'FORGPREP':
            d['ins'], d['ctrl'], d['alt'] = [(op, a)], ('JUMP', a), to
        else:
            d['ins'] = [(op, a)]
        return d


def roundtrip(code, consts):
    """decompile a main chunk -> (source, bytecode text of the source, fidelity report)"""
    return roundtrip_protos({0: dict(code=code, consts=dict(enumerate(consts)))})


def roundtrip_protos(protos):
    """the same for a program of several functions: {proto id: dict(code, consts, kids, np, nup)}, proto 0 is the main chunk"""
    dec = FakeDecoder(protos)
    text = render(dec, main_pid=0).text
    ok, msg, bytecode = compile_check(text)
    assert ok, '%s\n%s' % (msg, text)
    fd, path = tempfile.mkstemp(suffix='.bc.txt')
    try:
        with os.fdopen(fd, 'w', encoding='latin1') as f:
            f.write(bytecode)
        report = fidelity.compare(dec, path)
    finally:
        os.unlink(path)
    return text, bytecode, report


def test_value_moved_into_a_concat_slot_is_not_merged_with_an_older_value_of_that_register():
    """`items.Status = "PREFIX " .. product:upper() .. "!"`: the call is compiled above the three operand slots and its
    result is moved down into the middle one.  That register held `100` in an earlier branch; the old register-reuse rule
    joined the two values into one variable, which was declared inside the `if` and then assigned outside it (a global
    write)."""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0),
        I('GETTABLEKS', A=1, B=0, aux=0),            # local product = items.Product
        I('GETTABLEKS', A=2, B=0, aux=6),            # local flag = items.Flag
        I('JUMPIFNOT', to=6, A=2),                   # if flag then
        I('LOADN', A=5, rf=100),                     #   items.Count = 100          (R5 is also the middle concat slot)
        I('SETTABLEKS', A=5, B=0, aux=5),            # end
        I('LOADK', A=4, rf=2),                       # R4 = "PREFIX "
        I('SELF', A=7, B=1, aux=1),                  # R7, R8 = product.upper, product
        I('CALL', A=7, B=2, C=2),                    # R7 = product:upper()
        I('MOVE', A=5, B=7),                         # R5 = R7  (into the operand slot)
        I('LOADK', A=6, rf=3),                       # R6 = "!"
        I('CONCAT', A=3, B=4, C=6),
        I('SETTABLEKS', A=3, B=0, aux=4),            # items.Status = R3
        I('RETURN', A=0, B=1),
    ], [s('Product'), s('upper'), s('PREFIX '), s('!'), s('Status'), s('Count'), s('Flag')])
    assert re.search(r'\.Status = "PREFIX " \.\. \w+:upper\(\) \.\. "!"', text), text
    assert 'SETGLOBAL' not in bytecode, text
    assert report['differing'] == [], report['report']


def test_a_register_copy_that_really_reassigns_a_variable_is_still_one_variable():
    """the rule above must not break plain reassignment: `value = other` after a branch keeps one `value`"""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0),
        I('GETTABLEKS', A=1, B=0, aux=0),            # local value = items.A
        I('GETTABLEKS', A=2, B=0, aux=1),            # local other = items.B
        I('GETTABLEKS', A=3, B=0, aux=2),            # R3 = items.Flag
        I('JUMPIFNOT', to=6, A=3),                   # if R3 then
        I('MOVE', A=1, B=2),                         #   value = other
        I('SETTABLEKS', A=1, B=0, aux=3),            # end; items.Out = value
        I('RETURN', A=0, B=1),
    ], [s('A'), s('B'), s('Flag'), s('Out')])
    assert 'SETGLOBAL' not in bytecode, text
    assert len(re.findall(r'^local \w+ = ', text, re.M)) == 3 and re.search(r'^\t(\w+) = (\w+)$', text, re.M), text
    assert report['differing'] == [], report['report']


def test_a_dead_index_read_is_kept_because_it_can_raise_or_run_a_metamethod():
    """`local unused = items.Stock[key] or 0` is never read, but the original computed it (and the first upvalue a
    closure reads can depend on it), so it stays as `local _ = ...`"""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0),
        I('GETTABLEKS', A=1, B=0, aux=0),            # local key = items.Key
        I('GETTABLEKS', A=3, B=0, aux=1),            # R3 = items.Stock
        I('GETTABLE', A=2, B=3, C=1),                # R2 = R3[key]
        I('ORK', A=2, B=2, C=2),                     # R2 = R2 or 0        (nothing reads R2)
        I('GETIMPORT', A=3, aux=import_aux(3)),
        I('MOVE', A=4, B=1),
        I('CALL', A=3, B=2, C=1),                    # print(key)
        I('RETURN', A=0, B=1),
    ], [s('Key'), s('Stock'), num(0), s('print')])
    assert re.search(r'^\s*local _ = \w+\.Stock\[\w+\] or 0$', text, re.M), text
    assert report['differing'] == [], report['report']


def test_dead_constants_and_plain_copies_are_still_dropped():
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0),
        I('GETTABLEKS', A=1, B=0, aux=0),            # local key = items.Key
        I('LOADN', A=2, rf=7),                       # a constant nobody reads
        I('MOVE', A=3, B=1),                         # a copy nobody reads
        I('GETIMPORT', A=2, aux=import_aux(1)),
        I('MOVE', A=3, B=1),
        I('CALL', A=2, B=2, C=1),                    # print(key)
        I('RETURN', A=0, B=1),
    ], [s('Key'), s('print')])
    assert '_ =' not in text and '= 7' not in text, text
    assert report['differing'] == [], report['report']


# ------------------------------------------------------------------------------------------------ newer builds (FORGE v0.43)
def test_varargs_are_read_into_locals_packed_into_a_table_counted_and_returned():
    """`local a, b = ...`, `{...}`, `select("#", ...)` and `return ...` (GETVARARGS with a fixed and an open count)"""
    text, bytecode, report = roundtrip([
        I('GETVARARGS', A=0, B=3),                   # local a, b = ...
        I('NEWTABLE', A=2),
        I('GETVARARGS', A=3, B=0),                   # every argument, into R3 and up
        I('SETLIST', A=2, B=3, C=0, aux=1),          # local t = {...}
        I('GETIMPORT', A=3, aux=import_aux(0)),
        I('MOVE', A=4, B=0),
        I('MOVE', A=5, B=1),
        I('LENGTH', A=6, B=2),
        I('CALL', A=3, B=4, C=1),                    # print(a, b, #t)
        I('GETIMPORT', A=3, aux=import_aux(0)),
        I('GETIMPORT', A=4, aux=import_aux(1)),
        I('LOADK', A=5, rf=2),
        I('GETVARARGS', A=6, B=0),
        I('CALL', A=4, B=0, C=0),                    # select("#", ...)   (all its results are passed on)
        I('CALL', A=3, B=0, C=1),                    # print(select("#", ...))
        I('GETVARARGS', A=3, B=0),
        I('RETURN', A=3, B=0),                       # return ...
    ], [s('print'), s('select'), s('#')])
    assert re.search(r'^local (\w+), (\w+) = \.\.\.$', text, re.M), text
    assert '{...}' in text and 'select("#", ...)' in text, text
    assert re.search(r'^return \.\.\.$', text, re.M), text
    assert report['differing'] == [], report['report']


def test_register_forms_of_modulo_division_power_floor_division_and_the_logic_operators():
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),      # local a = x
        I('GETIMPORT', A=1, aux=import_aux(1)),      # local b = y
        I('MOD', A=2, B=0, C=1),
        I('DIV', A=3, B=0, C=1),
        I('POW', A=4, B=0, C=1),
        I('IDIV', A=5, B=0, C=1),
        I('AND', A=6, B=0, C=1),
        I('OR', A=7, B=0, C=1),
        I('GETIMPORT', A=8, aux=import_aux(2)),
        I('MOVE', A=9, B=2),
        I('MOVE', A=10, B=3),
        I('MOVE', A=11, B=4),
        I('MOVE', A=12, B=5),
        I('MOVE', A=13, B=6),
        I('MOVE', A=14, B=7),
        I('CALL', A=8, B=7, C=1),                    # print(a % b, a / b, a ^ b, a // b, a and b, a or b)
        I('RETURN', A=0, B=1),
    ], [s('x'), s('y'), s('print')])
    for op in ('%', '/', '^', '//', 'and', 'or'):
        assert re.search(r'\w %s \w' % re.escape(op) if op in ('and', 'or') else r'\w ?%s ?\w' % re.escape(op), text), (op, text)
    assert report['differing'] == [], report['report']


def test_whole_numbers_below_2_to_the_53_print_in_full_and_every_number_reads_back_as_itself():
    assert fmt_num(4503599627370496.0) == '4503599627370496'
    assert fmt_num(2.0 ** 53 - 1) == '9007199254740991'
    assert float(fmt_num(2.0 ** 60)) == 2.0 ** 60
    assert fmt_num(0.1) == '0.1' and fmt_num(-3.0) == '-3'
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('LOADK', A=1, rf=1),
        I('LOADK', A=2, rf=2),
        I('LOADK', A=3, rf=3),
        I('CALL', A=0, B=4, C=1),                    # print(4503599627370496, 9007199254740991, 2 ^ 54)
        I('RETURN', A=0, B=1),
    ], [s('print'), num(4503599627370496), num(9007199254740991), num(18014398509481984)])
    assert '4503599627370496' in text and '9007199254740991' in text, text
    assert report['differing'] == [], report['report']


def test_a_local_function_that_calls_itself_is_declared_as_one_local_function():
    """the closure captures its own register (CAPTURE VAL of the register it is stored in), which is the recursion"""
    text, bytecode, report = roundtrip_protos({
        0: dict(code=[
            I('DUPCLOSURE', A=0, rf=1),              # local function countdown(n)
            I('CAPTURE', A=0, B=0),                  #   (captures itself)
            I('GETIMPORT', A=1, aux=import_aux(0)),
            I('MOVE', A=2, B=0),
            I('LOADN', A=3, rf=3),
            I('CALL', A=2, B=2, C=0),                # countdown(3) ...
            I('CALL', A=1, B=0, C=1),                # print(countdown(3))
            I('RETURN', A=0, B=1),
        ], consts={0: s('print'), 1: num(1)}),       # the closure constant names proto 1
        1: dict(code=[
            I('LOADN', A=1, rf=0),
            I('JUMPIFNOTLT', to=6, A=1, aux=0),      # if n > 0 then
            I('GETUPVAL', A=1, B=0),
            I('SUBK', A=2, B=0, C=0),
            I('CALL', A=1, B=2, C=0),
            I('RETURN', A=1, B=0),                   #   return countdown(n - 1)
            I('LOADN', A=1, rf=0),                   # end
            I('RETURN', A=1, B=2),                   # return 0
        ], consts={0: num(1)}, np=1, nup=1),
    })
    m = re.search(r'^local function (\w+)\((\w+)\)$', text, re.M)
    assert m, text
    assert re.search(r'return %s\(%s - 1\)' % (m.group(1), m.group(2)), text), text
    assert 'local %s;' % m.group(1) not in text and '%s = function' % m.group(1) not in text, text
    assert report['differing'] == [], report['report']


def test_a_local_function_nobody_calls_is_kept():
    text, bytecode, report = roundtrip_protos({
        0: dict(code=[
            I('DUPCLOSURE', A=0, rf=1),
            I('GETIMPORT', A=1, aux=import_aux(0)),
            I('LOADK', A=2, rf=2),
            I('CALL', A=1, B=2, C=1),                # print("hi")
            I('RETURN', A=0, B=1),
        ], consts={0: s('print'), 1: num(1), 2: s('hi')}),
        1: dict(code=[I('LOADN', A=0, rf=1), I('RETURN', A=0, B=2)], consts={}),
    })
    assert re.search(r'local function \w+\(\)\n\s+return 1\nend', text), text
    assert report['differing'] == [], report['report']


def test_a_method_call_on_a_local_does_not_pull_the_local_s_call_in_behind_loads_made_before_it():
    """`local svc = g("HttpService")` then `writefile("name.json", svc:JSONEncode(items))`: the call arguments are
    started (writefile, the file name) before the SELF reads `svc` from its own register.  Writing `g(...)` inside the
    method call would run it after those loads, which compiles to different bytecode; a receiver in a register of its
    own (the SELF object is not the call's register) stays a local"""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('LOADK', A=1, rf=1),
        I('CALL', A=0, B=2, C=2),                    # local svc = g("HttpService")
        I('GETIMPORT', A=1, aux=import_aux(2)),
        I('LOADK', A=2, rf=3),
        I('GETIMPORT', A=5, aux=import_aux(4)),
        I('SELF', A=3, B=0, aux=5),
        I('CALL', A=3, B=3, C=0),                    # svc:JSONEncode(items)
        I('CALL', A=1, B=0, C=1),                    # writefile("name.json", ...)
        I('RETURN', A=0, B=1),
    ], [s('g'), s('HttpService'), s('writefile'), s('name.json'), s('items'), s('JSONEncode')])
    assert re.search(r'^local (\w+) = g\("HttpService"\)$', text, re.M), text
    assert text.index('g("HttpService")') < text.index('writefile('), text
    assert report['differing'] == [] and report['reordered'] == 0, report['report']


def test_a_method_call_on_a_value_made_in_the_call_s_own_register_is_still_written_in_one_expression():
    """`g():m()` computes the object straight into the register the call uses (SELF A == B), so there is no variable"""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('CALL', A=0, B=1, C=2),                    # g()
        I('SELF', A=0, B=0, aux=1),
        I('CALL', A=0, B=2, C=1),                    # :m()
        I('RETURN', A=0, B=1),
    ], [s('g'), s('m')])
    assert re.search(r'^g\(\):m\(\)$', text, re.M), text
    assert report['differing'] == [], report['report']


def fields_by_assignment(n):
    """`local t = {}`, then `t.k0 = f0()` ... `t.k{n-1} = f{n-1}()`, then `return t` (the compiler makes it NEWTABLE)"""
    code = [I('NEWTABLE', A=0, B=7)]
    consts = []
    for i in range(n):
        consts += [s('f%d' % i), s('k%d' % i)]
        code += [I('GETIMPORT', A=1, aux=import_aux(2 * i)), I('CALL', A=1, B=1, C=2), I('SETTABLEKS', A=1, B=0, aux=2 * i + 1)]
    code.append(I('RETURN', A=0, B=2))
    return code, consts


def test_fields_assigned_after_an_empty_table_stay_assignments():
    """a constructor of record fields is compiled to DUPTABLE, so NEWTABLE followed by SETTABLEKS was `{}` plus assignments"""
    text, bytecode, report = roundtrip(*fields_by_assignment(3))
    assert re.search(r'^local (\w+) = \{\}$', text, re.M), text
    assert re.search(r'^\w+\.k0 = f0\(\)$', text, re.M) and re.search(r'^\w+\.k2 = f2\(\)$', text, re.M), text
    assert 'NEWTABLE' in bytecode and 'DUPTABLE' not in bytecode, bytecode
    assert report['differing'] == [], report['report']


def test_a_table_template_is_folded_into_a_constructor():
    text, bytecode, report = roundtrip([
        I('DUPTABLE', A=0, rf=3),
        I('GETIMPORT', A=1, aux=import_aux(4)),
        I('CALL', A=1, B=1, C=2),
        I('SETTABLEKS', A=1, B=0, aux=0),
        I('GETIMPORT', A=1, aux=import_aux(5)),
        I('CALL', A=1, B=1, C=2),
        I('SETTABLEKS', A=1, B=0, aux=1),
        I('RETURN', A=0, B=2),
    ], [s('a'), s('b'), s('c'), ('t', [(0, -1), (1, -1)]), s('f'), s('g')])
    assert re.search(r'^return \{a = f\(\), b = g\(\)\}$', text, re.M), text
    assert 'DUPTABLE' in bytecode, bytecode
    assert report['differing'] == [], report['report']


@pytest.mark.parametrize('n, folded', [(32, False), (33, True)])
def test_table_constructor_size_limit_for_templates(n, folded):
    """a constructor with up to 32 record fields is DUPTABLE; with more it is NEWTABLE and indistinguishable from `{}` and
    assignments, which is then printed as the constructor"""
    text, bytecode, report = roundtrip(*fields_by_assignment(n))
    if folded:
        assert re.search(r'^(local \w+ =|return) \{\n\tk0 = f0\(\),\n\tk1 = f1\(\),\n', text, re.M), text
    else:
        assert re.search(r'^local \w+ = \{\}$', text, re.M), text
    assert report['differing'] == [], report['report']


def test_a_shared_tail_in_an_or_and_chain_becomes_one_expression():
    """`local x = (a or b and c) and game`: both paths end in the same `game` load"""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=1, aux=import_aux(0)),
        I('JUMPIF', to=6, A=1),                      # a is true: x = game
        I('GETIMPORT', A=0, aux=import_aux(1)),
        I('JUMPIFNOT', to=7, A=0),                   # b is false: x = b
        I('GETIMPORT', A=0, aux=import_aux(2)),
        I('JUMPIFNOT', to=7, A=0),                   # c is false: x = c
        I('GETIMPORT', A=0, aux=import_aux(3)),      # x = game
        I('GETIMPORT', A=1, aux=import_aux(4)),
        I('MOVE', A=2, B=0),
        I('CALL', A=1, B=2, C=1),                    # print(x)
        I('RETURN', A=0, B=1),
    ], [s('a'), s('b'), s('c'), s('game'), s('print')])
    assert re.search(r'= \(a or b and c\) and game$', text, re.M), text
    assert report['differing'] == [], report['report']


# ------------------------------------------------------------------------------------------------ shapes from a second v0.43.0 build
def test_a_call_cut_down_to_one_value_keeps_its_parentheses_in_the_last_place():
    """A call in the last place of an argument list, a table or a return passes on every result unless the compiler asked
    for one (CALL with C=2), so `print(a, (f()))`, `{f(), (f())}` and `return (f())` need their parentheses and
    `print(a, f())` must not get any"""
    consts = [s('print'), s('a'), s('f')]
    one, many = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('GETIMPORT', A=1, aux=import_aux(1)),
        I('GETIMPORT', A=2, aux=import_aux(2)),
        I('CALL', A=2, B=1, C=2),
        I('CALL', A=0, B=3, C=1),                    # print(a, (f()))
        I('RETURN', A=0, B=1),
    ], consts), roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('GETIMPORT', A=1, aux=import_aux(1)),
        I('GETIMPORT', A=2, aux=import_aux(2)),
        I('CALL', A=2, B=1, C=0),
        I('CALL', A=0, B=0, C=1),                    # print(a, f())
        I('RETURN', A=0, B=1),
    ], consts)
    assert re.search(r'^print\(a, \(f\(\)\)\)$', one[0], re.M), one[0]
    assert re.search(r'^print\(a, f\(\)\)$', many[0], re.M), many[0]
    table = roundtrip([
        I('NEWTABLE', A=0, C=2),
        I('GETIMPORT', A=1, aux=import_aux(2)),
        I('CALL', A=1, B=1, C=2),
        I('GETIMPORT', A=2, aux=import_aux(2)),
        I('CALL', A=2, B=1, C=2),
        I('SETLIST', A=0, B=1, C=3, aux=1),          # {f(), (f())}
        I('RETURN', A=0, B=2),
    ], consts)
    assert re.search(r'^return \{f\(\), \(f\(\)\)\}$', table[0], re.M), table[0]
    result = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(2)),
        I('CALL', A=0, B=1, C=2),
        I('RETURN', A=0, B=2),                       # return (f())
    ], consts)
    assert re.search(r'^return \(f\(\)\)$', result[0], re.M), result[0]
    for text, bytecode, report in (one, many, table, result):
        assert report['differing'] == [], report['report']


def test_a_value_made_by_and_that_is_tested_again_afterwards_stays_a_local():
    """`local v = a and b` then `if not v then ... end` builds the value in a register and tests that register; the
    compiler never does that for `if not (a and b)`, which is a chain of direct jumps.  One expression in the `if` would
    compile to those direct jumps instead"""
    consts = [s('a'), s('b'), s('print'), s('x')]
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('JUMPIFNOT', to=3, A=0),                   # local v = a and b
        I('GETIMPORT', A=0, aux=import_aux(1)),
        I('JUMPIF', to=7, A=0),                      # if not v then
        I('GETIMPORT', A=1, aux=import_aux(2)),
        I('LOADK', A=2, rf=3),
        I('CALL', A=1, B=2, C=1),                    #   print("x")
        I('RETURN', A=0, B=1),                       # end
    ], consts)
    m = re.search(r'^local (\w+) = a and b$', text, re.M)
    assert m and re.search(r'^if not %s then$' % m.group(1), text, re.M), text
    assert report['differing'] == [], report['report']
    direct, _, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('JUMPIFNOT', to=4, A=0),                   # if not (a and b) then
        I('GETIMPORT', A=0, aux=import_aux(1)),
        I('JUMPIF', to=7, A=0),
        I('GETIMPORT', A=0, aux=import_aux(2)),
        I('LOADK', A=1, rf=3),
        I('CALL', A=0, B=2, C=1),                    #   print("x")
        I('RETURN', A=0, B=1),                       # end
    ], consts)
    assert not re.search(r'^local ', direct, re.M), direct
    assert report['differing'] == [], report['report']


def metatable_program(template):
    """`return function(p1) return setmetatable({}, {__index = <closure>, __tostring = <closure>}) end`; the second table
    is a template (DUPTABLE) or, when the compiler did not make one, a NEWTABLE filled by assignments"""
    second = I('DUPTABLE', A=3, rf=3) if template else I('NEWTABLE', A=3, B=2)
    return {
        0: dict(code=[I('DUPCLOSURE', A=0, rf=0), I('RETURN', A=0, B=2)], consts={0: num(1)}),
        1: dict(code=[
            I('NEWTABLE', A=2),                      # the first argument
            second,                                  # the second argument
            I('NEWCLOSURE', A=4, rf=0),
            I('CAPTURE', A=0, B=0),
            I('SETTABLEKS', A=4, B=3, aux=0),        # __index = function() return p1 end
            I('NEWCLOSURE', A=4, rf=1),
            I('CAPTURE', A=0, B=0),
            I('SETTABLEKS', A=4, B=3, aux=1),        # __tostring = function() return p1 end
            I('GETIMPORT', A=1, aux=import_aux(2)),
            I('CALL', A=1, B=3, C=0),
            I('RETURN', A=1, B=0),
        ], consts={0: s('__index'), 1: s('__tostring'), 2: s('setmetatable'), 3: ('t', [(0, -1), (1, -1)])},
            np=1, kids=[2, 3]),
        2: dict(code=[I('GETUPVAL', A=0, B=0), I('RETURN', A=0, B=2)], consts={}, nup=1),
        3: dict(code=[I('GETUPVAL', A=0, B=0), I('RETURN', A=0, B=2)], consts={}, nup=1),
    }


def test_two_tables_that_are_both_arguments_are_made_in_the_order_they_were_compiled():
    """With the second table filled by assignments the compiler made `{}` first and the other table after it; both are
    locals here (a table that is built field by field cannot sit inside the call), so the first one is declared first"""
    text, bytecode, report = roundtrip_protos(metatable_program(template=False))
    m = re.search(r'^\s*local (\w+) = \{\}\n\s*local (\w+) = \{\}\n', text, re.M)
    assert m, text
    assert re.search(r'return setmetatable\(%s, %s\)' % m.groups(), text), text
    assert report['differing'] == [], report['report']


def test_a_table_template_filled_in_for_an_argument_is_folded_back_behind_the_empty_table_before_it():
    """With a template the second table is one constructor, so the whole thing is still `setmetatable({}, {...})`"""
    text, bytecode, report = roundtrip_protos(metatable_program(template=True))
    assert re.search(r'return setmetatable\(\{\}, \{__index = function', text), text
    assert 'local' not in text.replace('local function', ''), text
    assert report['differing'] == [], report['report']


def test_a_list_constructor_with_nested_tables_filled_in_afterwards_is_one_expression():
    """`{ {words = {"a", "b"}, paths = {"c"}}, {words = {"d"}, paths = {"e", "f"}} }`: the outer table is filled by one
    SETLIST at the end, after statements that complete the inner tables.  It must stay a pending value (the unfinished
    constructor), not become a variable that SETLIST can no longer write into"""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0, C=2),
        I('DUPTABLE', A=1, rf=9),
        I('NEWTABLE', A=2, C=2),
        I('LOADK', A=3, rf=2),
        I('LOADK', A=4, rf=3),
        I('SETLIST', A=2, B=3, C=3, aux=1),
        I('SETTABLEKS', A=2, B=1, aux=0),            # words = {"a", "b"}
        I('NEWTABLE', A=2, C=1),
        I('LOADK', A=3, rf=4),
        I('SETLIST', A=2, B=3, C=2, aux=1),
        I('SETTABLEKS', A=2, B=1, aux=1),            # paths = {"c"}
        I('DUPTABLE', A=2, rf=9),
        I('NEWTABLE', A=3, C=1),
        I('LOADK', A=4, rf=5),
        I('SETLIST', A=3, B=4, C=2, aux=1),
        I('SETTABLEKS', A=3, B=2, aux=0),            # words = {"d"}
        I('NEWTABLE', A=3, C=2),
        I('LOADK', A=4, rf=6),
        I('LOADK', A=5, rf=7),
        I('SETLIST', A=3, B=4, C=3, aux=1),
        I('SETTABLEKS', A=3, B=2, aux=1),            # paths = {"e", "f"}
        I('SETLIST', A=0, B=1, C=3, aux=1),
        I('RETURN', A=0, B=2),
    ], [s('words'), s('paths'), s('a'), s('b'), s('c'), s('d'), s('e'), s('f'), s('g'), ('t', [(0, -1), (1, -1)])])
    assert 'words = {"a", "b"}' in text and 'paths = {"e", "f"}' in text, text
    assert re.search(r'^return \{', text, re.M), text
    assert report['differing'] == [], report['report']


def test_a_list_constructor_around_a_table_built_by_assignments_is_still_one_constructor():
    """`{ t }` where `t` is a NEWTABLE that fields are assigned into: those assignments come before the SETLIST of the
    outer table, but they must not turn the outer table into a variable (SETLIST cannot write into one).  The outer table
    is made after the inner one here, which the bytecode check reports, but the decompiler must not fail"""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0, C=1),
        I('NEWTABLE', A=1),
        I('LOADN', A=2, rf=1),
        I('SETTABLEKS', A=2, B=1, aux=0),            # t.x = 1
        I('SETLIST', A=0, B=1, C=2, aux=1),
        I('RETURN', A=0, B=2),
    ], [s('x')])
    assert re.search(r'^return \{\w+\}$', text, re.M), text


# ------------------------------------------------------------------------------------------------ loops
def test_a_while_loop_that_ends_with_a_generic_for_loop():
    """`while #list > 0 do local x = table.remove(list, 1) for _, v in iter(x) do print(v) end end`: when the for loop is
    done it goes straight back to the top of the while loop, which the loop detection used to ignore (it never found the
    while loop and the structuring went on for ever)"""
    text, bytecode, report = roundtrip([
        I('GETVARARGS', A=0, B=2),                   # local list = ...
        I('LENGTH', A=1, B=0),                       # while #list > 0 do
        I('LOADN', A=2, rf=0),
        I('JUMPIFNOTLT', to=17, A=2, aux=1),
        I('GETIMPORT', A=1, aux=import_aux(0, 1)),
        I('MOVE', A=2, B=0),
        I('LOADN', A=3, rf=1),
        I('CALL', A=1, B=3, C=2),                    #   local x = table.remove(list, 1)
        I('GETIMPORT', A=2, aux=import_aux(2)),
        I('MOVE', A=3, B=1),
        I('CALL', A=2, B=2, C=4),
        I('FORGPREP', to=15, A=2),                   #   for _, v in iter(x) do
        I('GETIMPORT', A=7, aux=import_aux(3)),
        I('MOVE', A=8, B=6),
        I('CALL', A=7, B=2, C=1),                    #     print(v)
        I('FORGLOOP', to=12, A=2, aux=2),            #   end
        I('JUMP', to=1),                             # end
        I('RETURN', A=0, B=1),
    ], [s('table'), s('remove'), s('iter'), s('print')])
    assert re.search(r'^while #\w+ > 0 do$', text, re.M), text
    assert re.search(r'^\tfor _, \w+ in iter\(\w+\) do$', text, re.M), text
    assert report['differing'] == [], report['report']


def test_a_while_loop_that_ends_with_a_numeric_for_loop():
    text, bytecode, report = roundtrip([
        I('GETVARARGS', A=0, B=2),                   # local n = ...
        I('LOADN', A=1, rf=0),                       # while n > 0 do
        I('JUMPIFNOTLT', to=13, A=1, aux=0),
        I('SUBK', A=0, B=0, C=1),                    #   n = n - 1
        I('LOADN', A=3, rf=1),
        I('LOADN', A=1, rf=3),
        I('LOADN', A=2, rf=1),
        I('FORNPREP', to=12, A=1),                   #   for i = 1, 3 do
        I('GETIMPORT', A=4, aux=import_aux(0)),
        I('MOVE', A=5, B=3),
        I('CALL', A=4, B=2, C=1),                    #     print(i)
        I('FORNLOOP', to=8, A=1),                    #   end
        I('JUMP', to=1),                             # end
        I('RETURN', A=0, B=2),                       # return n
    ], [s('print'), num(1)])
    assert re.search(r'^while \w+ > 0 do$', text, re.M), text
    assert re.search(r'^\tfor \w+ = 1, 3 do$', text, re.M), text
    assert report['differing'] == [], report['report']


@pytest.mark.parametrize('name', ['getgc', 'toclipboard', 'runonactor', 'hookmetamethod', 'rconsoleprint', 'crypt', 'json',
                                  'info'])
def test_a_local_never_takes_the_name_of_an_executor_function(name):
    """The Real executor documents 232 functions (plus aliases and library tables such as `crypt` and `json`).  A local
    named like one of them hides it for the rest of the scope, so `local getgc = items.getgc` is written `getgc2`."""
    text, bytecode, report = roundtrip([
        I('NEWTABLE', A=0),
        I('GETTABLEKS', A=1, B=0, aux=0),            # local <name> = items.<name>
        I('SETTABLEKS', A=1, B=0, aux=1),            # items.A = <name>
        I('SETTABLEKS', A=1, B=0, aux=2),            # items.B = <name>
        I('RETURN', A=0, B=1),
    ], [s(name), s('A'), s('B')])
    assert re.search(r'^local %s2 = \w+\.%s$' % (name, name), text, re.M), text
    assert not re.search(r'\blocal %s\b' % name, text), text
    assert report['differing'] == [], report['report']


def test_a_local_named_after_a_function_the_script_calls_does_not_hide_it():
    """`local fetch = fetch("a")` would turn the next `fetch(...)` into a call of a string.  Every function the script
    imports is kept free for the whole script, even when it is not on the list of known globals."""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)),
        I('LOADK', A=1, rf=1),
        I('CALL', A=0, B=2, C=2),                    # local x = fetch("a")     (the name hint of x is `fetch`)
        I('GETIMPORT', A=1, aux=import_aux(0)),
        I('MOVE', A=2, B=0),
        I('CALL', A=1, B=2, C=1),                    # fetch(x)
        I('GETIMPORT', A=1, aux=import_aux(0)),
        I('MOVE', A=2, B=0),
        I('CALL', A=1, B=2, C=1),                    # fetch(x)
        I('RETURN', A=0, B=1),
    ], [s('fetch'), s('a')])
    assert re.search(r'^local fetch2 = fetch\("a"\)$', text, re.M), text
    assert len(re.findall(r'^fetch\(fetch2\)$', text, re.M)) == 2, text
    assert report['differing'] == [], report['report']


# ------------------------------------------------------------------------------------------------ same instructions in the same order
def test_a_value_read_once_below_the_register_of_its_result_stays_a_local():
    """`local pi = math.pi` ... `print("x")` ... `local m = pi * 2`: the compiler gives the result of `pi * 2` its register
    before it computes the operands, so an operand that sits below the result was computed earlier, for a variable.
    Writing `return math.pi * 2` would move the import behind the call (the bytecode check calls that "reordered")."""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0, 1)),   # local n = math.pi
        I('GETIMPORT', A=1, aux=import_aux(2)),
        I('LOADK', A=2, rf=3),
        I('CALL', A=1, B=2, C=1),                    # print("x")
        I('MULK', A=1, B=0, C=4),                    # local m = n * 2
        I('RETURN', A=1, B=2),                       # return m
    ], [s('math'), s('pi'), s('print'), s('x'), num(2)])
    assert re.search(r'^local (\w+) = math\.pi\nprint\("x"\)\nreturn \1 \* 2$', text, re.M), text
    assert report['identical'] == report['functions_orig'], report['report']
    direct, _, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(2)),
        I('LOADK', A=1, rf=3),
        I('CALL', A=0, B=2, C=1),                    # print("x")
        I('GETIMPORT', A=0, aux=import_aux(0, 1)),   # the import is computed for the multiplication itself
        I('MULK', A=0, B=0, C=4),
        I('RETURN', A=0, B=2),                       # return math.pi * 2
    ], [s('math'), s('pi'), s('print'), s('x'), num(2)])
    assert direct.endswith('print("x")\nreturn math.pi * 2\n'), direct
    assert report['identical'] == report['functions_orig'], report['report']


def test_a_value_made_by_and_or_that_is_read_once_below_the_register_of_its_result_stays_a_local():
    """`local v = a and b or c` then `print(v + g())`: the value has several definitions and the test of the middle one is
    one more read of the same variable, but the only real read is the addition, whose result register is above it.  Written
    as `print((a and b or c) + g())` the three global loads and the tests would move behind the load of `print`."""
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=1, aux=import_aux(0)),      # a
        I('JUMPIFNOT', to=4, A=1),
        I('GETIMPORT', A=0, aux=import_aux(1)),      # b
        I('JUMPIF', to=5, A=0),                      # local v = a and b or c
        I('GETIMPORT', A=0, aux=import_aux(2)),      # c
        I('GETIMPORT', A=1, aux=import_aux(3)),      # print(
        I('GETIMPORT', A=3, aux=import_aux(4)),
        I('CALL', A=3, B=1, C=2),                    #   g()
        I('ADD', A=2, B=0, C=3),                     #   v + g()
        I('CALL', A=1, B=2, C=1),                    # )
        I('RETURN', A=0, B=1),
    ], [s('a'), s('b'), s('c'), s('print'), s('g')])
    assert re.search(r'^local (\w+) = a and b or c\nprint\(\1 \+ g\(\)\)$', text, re.M), text
    assert report['identical'] == report['functions_orig'], report['report']


def test_a_variable_that_a_loop_assigns_and_returns_keeps_its_assignment():
    """`local flag = false`, then inside a loop `flag = true return flag`: the temporaries of a `for` loop sit above the
    loop's own registers, so a write to a lower register is an assignment to a variable declared before the loop.  Printed
    as `return true` the compiler would see a variable that is never assigned and turn the `return flag` after the loop
    into `return false`."""
    text, bytecode, report = roundtrip([
        I('LOADB', A=0, B=0),                        # local flag = false
        I('LOADN', A=3, rf=1),
        I('LOADN', A=1, rf=3),
        I('LOADN', A=2, rf=1),
        I('FORNPREP', to=12, A=1),                   # for i = 1, 3 do
        I('GETIMPORT', A=4, aux=import_aux(0)),
        I('MOVE', A=5, B=3),
        I('CALL', A=4, B=2, C=2),                    #   local r = f(i)
        I('JUMPIFNOT', to=11, A=4),                  #   if r then
        I('LOADB', A=0, B=1),                        #     flag = true
        I('RETURN', A=0, B=2),                       #     return flag
        I('FORNLOOP', to=5, A=1),                    # end
        I('RETURN', A=0, B=2),                       # return flag
    ], [s('f')])
    assert re.search(r'^local (\w+) = false\n.*\n\t\t\1 = true\n\t\treturn \1\n\tend\nend\n\nreturn \1$', text, re.M | re.S), text
    assert report['identical'] == report['functions_orig'], report['report']


def test_a_return_that_ends_an_if_branch_is_kept_when_the_bytecode_has_one_there():
    """`if not x then error("e") return end` is two RETURN instructions (the branch and the end of the function) and
    `if not x then error("e") end` is one.  The bytecode check counts them, so the branch keeps its `return` only when the
    program had one; the last return of the function itself is implicit and is never printed."""
    with_return, _, report = roundtrip([
        I('GETVARARGS', A=0, B=2),                   # local x = ...
        I('JUMPIF', to=6, A=0),                      # if not x then
        I('GETIMPORT', A=1, aux=import_aux(0)),
        I('LOADK', A=2, rf=1),
        I('CALL', A=1, B=2, C=1),                    #   error("e")
        I('RETURN', A=0, B=1),                       #   return
        I('RETURN', A=0, B=1),                       # end
    ], [s('error'), s('e')])
    assert re.search(r'^\treturn$', with_return, re.M) and with_return.endswith('\nend\n'), with_return
    assert report['identical'] == report['functions_orig'], report['report']
    without, _, report = roundtrip([
        I('GETVARARGS', A=0, B=2),                   # local x = ...
        I('JUMPIF', to=5, A=0),                      # if not x then
        I('GETIMPORT', A=1, aux=import_aux(0)),
        I('LOADK', A=2, rf=1),
        I('CALL', A=1, B=2, C=1),                    #   error("e")
        I('RETURN', A=0, B=1),                       # end
    ], [s('error'), s('e')])
    assert 'return' not in without, without
    assert report['identical'] == report['functions_orig'], report['report']


# ------------------------------------------------------------------------------------------------ blank lines
def test_a_blank_line_goes_around_every_block_and_small_statements_stay_together():
    text, bytecode, report = roundtrip([
        I('GETIMPORT', A=0, aux=import_aux(0)), I('LOADK', A=1, rf=1), I('CALL', A=0, B=2, C=1),      # print("a")
        I('GETIMPORT', A=0, aux=import_aux(0)), I('LOADK', A=1, rf=2), I('CALL', A=0, B=2, C=1),      # print("b")
        I('LOADN', A=2, rf=1), I('LOADN', A=0, rf=3), I('LOADN', A=1, rf=1),
        I('FORNPREP', to=14, A=0),                                                                    # for i = 1, 3 do
        I('GETIMPORT', A=3, aux=import_aux(0)), I('MOVE', A=4, B=2), I('CALL', A=3, B=2, C=1),         #   print(i)
        I('FORNLOOP', to=10, A=0),                                                                    # end
        I('GETIMPORT', A=0, aux=import_aux(0)), I('LOADK', A=1, rf=3), I('CALL', A=0, B=2, C=1),      # print("c")
        I('GETIMPORT', A=0, aux=import_aux(0)), I('LOADK', A=1, rf=4), I('CALL', A=0, B=2, C=1),      # print("d")
        I('RETURN', A=0, B=1),
    ], [s('print'), s('a'), s('b'), s('c'), s('d')])
    assert text == 'print("a")\nprint("b")\n\nfor i = 1, 3 do\n\tprint(i)\nend\n\nprint("c")\nprint("d")\n', text
    assert report['identical'] == report['functions_orig'], report['report']


def test_two_guard_clauses_in_a_row_are_apart_but_a_guard_next_to_a_small_statement_is_not():
    """`if not p1 then return end` is a guard: short, ends in `return`, inside a function.  Two of them in a row would run
    together into a wall, so they get a blank line; the plain statement after the last one stays close to it."""
    text, bytecode, report = roundtrip_protos({
        0: dict(code=[I('DUPCLOSURE', A=0, rf=0), I('RETURN', A=0, B=2)], consts={0: num(1)}),
        1: dict(code=[
            I('JUMPIF', to=2, A=0),
            I('RETURN', A=0, B=1),                   # if not p1 then return end
            I('JUMPIF', to=4, A=1),
            I('RETURN', A=0, B=1),                   # if not p2 then return end
            I('GETIMPORT', A=2, aux=import_aux(0)),
            I('LOADK', A=3, rf=1),
            I('CALL', A=2, B=2, C=1),                # print("x")
            I('RETURN', A=0, B=1),
        ], consts={0: s('print'), 1: s('x')}, np=2),
    })
    assert text == ('return function(p1, p2)\n\tif not p1 then\n\t\treturn\n\tend\n\n\tif not p2 then\n\t\treturn\n\tend\n'
                    '\tprint("x")\nend\n'), text
    assert report['identical'] == report['functions_orig'], report['report']
