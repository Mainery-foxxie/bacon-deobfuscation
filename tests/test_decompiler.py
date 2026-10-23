"""Decompiler tests on small hand-assembled VM programs, so they run without a protected script.

Each program uses the register layout that the Roblox Luau compiler produced in a real script that once exposed a
bug.  The result is rendered, compiled again with luau-compile and compared with the original through the same
bytecode check the engine runs on every job."""
import os
import re
import tempfile

import pytest

from bgdeobf import fidelity
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

    def decode(self, pid, rid):
        op, fields, to = self.code[pid][rid]
        a = dict(A=0, B=0, C=0, rf=0, Hf=0, aux=0)
        a.update(fields)
        d = dict(kind=op, sem=0, ins=[], ctrl=None, nxt=rid + 1, alt=NONE, rid=rid)
        if op == 'RETURN':
            d['ctrl'], d['nxt'] = (op, a), NONE
        elif op in COND or op == 'JUMP':
            d['ctrl'], d['alt'] = (op, a), to
        else:
            d['ins'] = [(op, a)]
        return d


def roundtrip(code, consts):
    """decompile a main chunk -> (source, bytecode text of the source, fidelity report)"""
    dec = FakeDecoder({0: dict(code=code, consts=dict(enumerate(consts)))})
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
