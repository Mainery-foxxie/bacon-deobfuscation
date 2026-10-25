"""Decoder tests on a hand-written VM dump, for the record layouts of newer builds (FORGE v0.43):

  * every register of a function is offset by a per-function number (the dump's `rs`)
  * a closure record carries its capture descriptors itself
  * a loop can be one record that holds the whole loop (iterator check, loop control and body)

The dump, the field layout and the handler map are made up here, so these tests run without a protected script.  The last
test decodes a small program through the real Decoder and checks the decompiled source against the original through the
same bytecode comparison the engine runs on every job."""
import os
import re
import tempfile

import pytest

from bgdeobf import fidelity
from bgdeobf.classify import Mapping
from bgdeobf.decode import NONE, DecodeError, Decoder, unshift_args
from bgdeobf.dump import parse_dump
from bgdeobf.engine import compile_check
from bgdeobf.render import render

# made-up operation numbers
LOADN, GETIMPORT, CALL, DUPCLOSURE, FORNPREP, RETURN, MOVE, GETUPVAL = 20, 21, 22, 23, 24, 25, 26, 27
NAMES = dict(LOADN=LOADN, GETIMPORT=GETIMPORT, CALL=CALL, DUPCLOSURE=DUPCLOSURE, FORNPREP=FORNPREP, RETURN=RETURN,
             MOVE=MOVE, GETUPVAL=GETUPVAL)
TAIL_LOOP, HEAD_LOOP, PRE_TAIL_LOOP = 9, 10, 11     # `sem` of the dedicated loop handlers
LOOP_TYPE = 4                                       # their handler type


def import_aux(*name_ids):
    aux = len(name_ids) << 30
    for i, k in enumerate(name_ids):
        aux |= k << (20 - 10 * i)
    return aux


def T(op, A=0, B=0, C=0, rf=0, hf=0, aux=0, nxt=NONE, alt=NONE):
    """the raw field table of a record (the order the dump has: op, ef, A, B, C, RF, HF, AUX, NXT, ALT)"""
    return [op, 0, A, B, C, rf, hf, aux, nxt, alt]


def part(A=0, B=0, C=0, rf=0, hf=0, aux=0):
    """one part of a fused record, in the fused field order (A, B, C, RF, HF, AUX)"""
    return [A, B, C, rf, hf, aux]


def dump_text(protos):
    """protos: {pid: dict(rs, np, va, nup, kids, consts, recs)}; a record is (sem, ded, T, parts)"""
    lines = ['N %d xorkey 7 main 0' % len(protos)]
    for pid, p in sorted(protos.items()):
        lines.append('P %d np=%d va=%d nup=%d ms=16 entry=0 df=0 vf=0 nrec=%d rs=%d' % (
            pid, p.get('np', 0), p.get('va', 0), p.get('nup', 0), len(p['recs']), p.get('rs', 0)))
        lines.append('H %d %s' % (pid, ','.join(str(k) for k in p.get('kids', []))))
        for cid, (ty, v) in sorted(p.get('consts', {}).items()):
            lines.append('C %d %d %s %s' % (pid, cid, ty, v.encode().hex() if ty == 's' else v))
        for rid, (sem, ded, t, parts) in enumerate(p['recs']):
            lines.append('R %d %d sem=%d ded=%s pat=- raw=%s parts=%s' % (
                pid, rid, sem, ded, ','.join(str(x) for x in t), ';'.join(','.join(str(x) for x in pr) for pr in parts)))
    return '\n'.join(lines) + '\n'


def mapping(loops=None):
    m = Mapping()
    m.simple = {LOADN: 'LOADN', GETIMPORT: 'GETIMPORT', MOVE: 'MOVE', GETUPVAL: 'GETUPVAL'}
    m.hot = {CALL: 'CALL', DUPCLOSURE: 'DUPCLOSURE'}
    m.ctrl = {FORNPREP: 'FORNPREP', RETURN: 'RETURN'}
    for sem, loop in (loops or {}).items():
        m.ded[(LOOP_TYPE, sem)] = {'loop': loop}
    return m


def decoder(protos, loops=None):
    return Decoder(parse_dump(dump_text(protos)), mapping(loops), {0: tuple(range(1, 11))}, {0: tuple(range(1, 7))}, -1)


def rec(op, sem=0, **fields):
    return (sem, '-', T(op, **fields), [])


# ------------------------------------------------------------------------------------------------ register offset
def test_the_register_offset_is_taken_off_every_register_operand_and_only_those():
    a = dict(A=13, B=14, C=15, rf=7, Hf=0, aux=16)
    assert unshift_args('MOVE', a, 10) == dict(a, A=3, B=4)
    assert unshift_args('ADD', a, 10) == dict(a, A=3, B=4, C=5)
    assert unshift_args('JUMPIFEQ', a, 10) == dict(a, A=3, aux=6)         # the second register of the comparison is in aux
    assert unshift_args('SUBRK', a, 10) == dict(a, A=3, C=5)              # B is a constant
    assert unshift_args('GETUPVAL', a, 10) == dict(a, A=3)                # B is an upvalue number
    assert unshift_args('RETURN', a, 10) == dict(a, A=3)                  # B is a count
    assert unshift_args('JUMP', a, 10) == a
    assert unshift_args('CAPTURE', dict(a, A=0), 10) == dict(a, A=0, B=4)   # capture of a register
    assert unshift_args('CAPTURE', dict(a, A=1), 10) == dict(a, A=1, B=4)
    assert unshift_args('CAPTURE', dict(a, A=2), 10) == dict(a, A=2)        # capture of an upvalue: B is not a register


def test_an_operation_without_a_register_layout_is_an_error_when_a_function_has_an_offset():
    with pytest.raises(DecodeError):
        unshift_args('NOT_A_REAL_OP', dict(A=1, B=2, C=3, rf=0, Hf=0, aux=0), 3)


def test_records_of_a_function_with_an_offset_decode_to_the_registers_the_compiler_used():
    dec = decoder({0: dict(rs=3, recs=[
        rec(LOADN, A=3 + 0, rf=5, nxt=1),
        rec(MOVE, A=3 + 4, B=3 + 1, nxt=2),
        rec(RETURN, A=3 + 2, B=2),
    ])})
    d0, d1, d2 = (dec.decode(0, i) for i in range(3))
    assert d0['ins'] == [('LOADN', dict(A=0, B=0, C=0, rf=5, Hf=0, aux=0))]
    assert d1['ins'][0][0] == 'MOVE' and (d1['ins'][0][1]['A'], d1['ins'][0][1]['B']) == (4, 1)
    assert d2['ctrl'][0] == 'RETURN' and (d2['ctrl'][1]['A'], d2['ctrl'][1]['B']) == (2, 2)
    assert d0['nxt'] == 1 and d1['nxt'] == 2 and d2['nxt'] == NONE


def test_a_function_without_an_offset_is_left_alone():
    dec = decoder({0: dict(recs=[rec(MOVE, A=7, B=1, nxt=1), rec(RETURN, A=0, B=1)])})
    d = dec.decode(0, 0)
    assert (d['ins'][0][1]['A'], d['ins'][0][1]['B']) == (7, 1)


# ------------------------------------------------------------------------------------------------ closures
def closure_proto(rs, captures):
    """one DUPCLOSURE record whose parts after the first are the capture descriptors (raw field order, like any record)"""
    parts = [T(0)] + [T(0, A=kind, B=idx) for kind, idx in captures]
    return dict(rs=rs, consts={2: ('n', 1)}, recs=[
        (0, '-', T(DUPCLOSURE, A=rs + 1, rf=2, nxt=1), parts),
        rec(RETURN, A=rs + 1, B=2),
    ])


def test_capture_descriptors_inside_a_closure_record_are_handed_on_as_capture_records():
    rs = 3
    dec = decoder({0: closure_proto(rs, [(0, rs + 0), (1, rs + 2), (2, 1)]), 1: dict(nup=3, recs=[rec(RETURN, A=0, B=1)])})
    d = dec.decode(0, 0)
    assert d['ins'][0][0] == 'DUPCLOSURE' and d['ins'][0][1]['A'] == 1
    ids = [((q + 1) << 40) | 0 for q in range(3)]
    assert d['nxt'] == ids[0]
    extra = d['extra']
    assert [vr for vr, _ in extra] == ids
    got = [(vd['kind'], vd['ins'][0][0], vd['ins'][0][1]['A'], vd['ins'][0][1]['B'], vd['nxt'], vd['alt']) for _, vd in extra]
    assert got == [('CAPTURE', 'CAPTURE', 0, 0, ids[1], NONE),            # by value, R0: the offset is gone
                   ('CAPTURE', 'CAPTURE', 1, 2, ids[2], NONE),            # by reference, R2
                   ('CAPTURE', 'CAPTURE', 2, 1, 1, NONE)]                 # an upvalue (number 1): not a register; the chain ends at nxt


def test_decode_all_lists_the_expanded_records_too():
    dec = decoder({0: closure_proto(0, [(0, 0)]), 1: dict(nup=1, recs=[rec(RETURN, A=0, B=1)])})
    D = dec.decode_all(0)
    assert sorted(D) == [0, 1, (1 << 40) | 0]
    assert D[(1 << 40) | 0]['kind'] == 'CAPTURE'


def test_a_closure_record_without_capture_parts_is_unchanged():
    dec = decoder({0: dict(consts={2: ('n', 1)}, recs=[rec(DUPCLOSURE, A=0, rf=2, nxt=1), rec(RETURN, A=0, B=1)]),
                   1: dict(recs=[rec(RETURN, A=0, B=1)])})
    d = dec.decode(0, 0)
    assert d['nxt'] == 1 and not d.get('extra')


# ------------------------------------------------------------------------------------------------ fused loops
def loop_dec(loop, parts, sem):
    return decoder({0: dict(recs=[
        (sem, str(LOOP_TYPE), T(0, nxt=1), parts),
        rec(RETURN, A=0, B=1),
    ])}, loops={sem: loop})


def test_a_loop_that_checks_its_iterator_first_becomes_entry_check_and_body_records():
    """pre-steps, then the check (FORGLOOP-like, which leaves the loop when it fails) and the body, which jumps back"""
    loop = dict(pre=[('LOADN', 0, {}, False)], head=[('FORGLOOP', 1, {}, True)], body=[('MOVE', 2, {}, False)], tail=[])
    dec = loop_dec(loop, [part(A=4, rf=1), part(A=5, C=2), part(A=6, B=7)], HEAD_LOOP)
    d = dec.decode(0, 0)
    head, body = (1 << 40) | 0, (2 << 40) | 0
    assert [n for n, _ in d['ins']] == ['LOADN'] and d['ctrl'][0] == 'JUMP' and d['alt'] == head and d['nxt'] == 1
    (h_id, h), (b_id, b) = d['extra']
    assert (h_id, b_id) == (head, body)
    assert h['ins'] == [] and h['ctrl'][0] == 'FORGLOOP' and h['ctrl'][1]['A'] == 5 and h['nxt'] == 1 and h['alt'] == body
    assert [n for n, _ in b['ins']] == ['MOVE'] and b['ctrl'][0] == 'JUMP' and b['alt'] == head


def test_a_loop_with_nothing_in_its_body_checks_itself_again():
    loop = dict(pre=[], head=[('FORGLOOP', 0, {}, True)], body=[], tail=[])
    d = loop_dec(loop, [part(A=5)], HEAD_LOOP).decode(0, 0)
    (h_id, h), = d['extra']
    assert d['alt'] == h_id and h['alt'] == h_id and h['nxt'] == 1


def test_a_loop_whose_body_comes_before_the_control_is_one_record_that_jumps_to_itself():
    loop = dict(pre=[], head=[], body=[('MOVE', 0, {}, False), ('MOVE', 1, {}, False)], tail=[('FORNLOOP', 2, {}, True)])
    d = loop_dec(loop, [part(A=1, B=2), part(A=3, B=4), part(A=0)], TAIL_LOOP).decode(0, 0)
    assert [n for n, _ in d['ins']] == ['MOVE', 'MOVE'] and d['ctrl'][0] == 'FORNLOOP'
    assert d['nxt'] == 1 and d['alt'] == 0 and not d.get('extra')


def test_a_loop_with_pre_steps_and_the_control_last_runs_the_pre_steps_once():
    loop = dict(pre=[('LOADN', 0, {}, False)], head=[], body=[('MOVE', 1, {}, False)], tail=[('FORNLOOP', 2, {}, True)])
    d = loop_dec(loop, [part(A=1, rf=2), part(A=3, B=4), part(A=0)], PRE_TAIL_LOOP).decode(0, 0)
    loop_id = (1 << 40) | 0
    assert [n for n, _ in d['ins']] == ['LOADN'] and d['ctrl'] is None and d['nxt'] == loop_id
    (l_id, l), = d['extra']
    assert l_id == loop_id and [n for n, _ in l['ins']] == ['MOVE'] and l['ctrl'][0] == 'FORNLOOP'
    assert l['nxt'] == 1 and l['alt'] == loop_id


def test_a_fused_loop_with_too_few_parts_is_an_error():
    loop = dict(pre=[], head=[], body=[('MOVE', 0, {}, False)], tail=[('FORNLOOP', 3, {}, True)])
    with pytest.raises(DecodeError):
        loop_dec(loop, [part(A=1, B=2)], TAIL_LOOP).decode(0, 0)


# ------------------------------------------------------------------------------------------------ the whole way
def test_a_program_with_offsets_a_fused_closure_and_a_fused_loop_decompiles_to_the_same_bytecode():
    """local x = g()                         R0
       local f = function() return x end     R1, captures R0 by value
       for i = 1, 3 do print(i) end          R2..R4 control, R5 print, R6 copy of i
       return f"""
    S = 3
    main = dict(rs=S, consts={0: ('s', 'g'), 1: ('s', 'print'), 2: ('n', 1)}, recs=[
        rec(GETIMPORT, A=S + 0, aux=import_aux(0), nxt=1),
        rec(CALL, A=S + 0, B=1, C=2, nxt=2),
        (0, '-', T(DUPCLOSURE, A=S + 1, rf=2, nxt=3), [T(0), T(0, A=0, B=S + 0)]),          # the capture is part of the record
        rec(LOADN, A=S + 4, rf=1, nxt=4),
        rec(LOADN, A=S + 2, rf=3, nxt=5),
        rec(LOADN, A=S + 3, rf=1, nxt=6),
        rec(FORNPREP, A=S + 2, nxt=7, alt=8),
        (TAIL_LOOP, str(LOOP_TYPE), T(0, nxt=8), [                                           # the whole loop in one record
            part(A=S + 5, aux=import_aux(1)),                                                # GETIMPORT print
            part(A=S + 6, B=S + 4),                                                          # MOVE i
            part(A=S + 5, B=2, C=1),                                                         # CALL print(i)
            part(A=S + 2),                                                                   # FORNLOOP
        ]),
        rec(RETURN, A=S + 1, B=2),
    ])
    child = dict(rs=5, nup=1, recs=[rec(GETUPVAL, A=5 + 0, B=0, nxt=1), rec(RETURN, A=5 + 0, B=2)])
    loop = dict(pre=[], head=[], tail=[('FORNLOOP', 3, {}, True)],
                body=[('GETIMPORT', 0, {}, False), ('MOVE', 1, {}, False), ('CALL', 2, {}, False)])
    dec = decoder({0: main, 1: child}, loops={TAIL_LOOP: loop})

    text = render(dec, main_pid=0).text
    ok, msg, bytecode = compile_check(text)
    assert ok, '%s\n%s' % (msg, text)
    assert re.search(r'^local (\w+) = g\(\)$', text, re.M), text
    assert re.search(r'^for (\w+) = 1, 3 do\n\tprint\(\1\)\nend$', text, re.M), text
    fd, path = tempfile.mkstemp(suffix='.bc.txt')
    try:
        with os.fdopen(fd, 'w', encoding='latin1') as f:
            f.write(bytecode)
        report = fidelity.compare(dec, path)
    finally:
        os.unlink(path)
    assert report['differing'] == [] and report['identical'] == 2, '%s\n%s' % (text, report['report'])
