"""Instruction decoder: turns dumped VM records into Luau-bytecode style instructions using a classified handler map."""
from .classify import RAWF

NONE = 4294967295

# operations that end a record with a conditional / loop transfer (the node carries `nxt` and `alt`)
CTRL_OPS = ('RETURN', 'FORNLOOP', 'FORGLOOP', 'JUMPIFNOT', 'JUMPIF', 'JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT',
            'JUMPIFNOTLT', 'JUMPIFNOTLE', 'JUMPXEQKB', 'JUMPXEQK', 'JUMPXEQKNIL', 'FORNPREP')
DATA_OPS = ('CALL', 'SETLIST', 'NEWCLOSURE', 'DUPCLOSURE', 'CLOSEUPVALS')


# operand fields that hold a register, per instruction name.  Newer builds encode every register of a function with a
# per-function offset (the dump's `rs`), which is taken off again after decoding.
_ARITH_K = ('ADDK', 'SUBK', 'MULK', 'DIVK', 'MODK', 'POWK', 'IDIVK')
REG_FIELDS = {}
for _n in ('LOADK', 'LOADN', 'LOADB', 'LOADNIL', 'GETIMPORT', 'GETUPVAL', 'SETUPVAL', 'NEWTABLE', 'DUPTABLE', 'NEWCLOSURE',
           'DUPCLOSURE', 'CALL', 'RETURN', 'JUMPIFNOT', 'JUMPIF', 'JUMPXEQK', 'JUMPXEQKB', 'JUMPXEQKNIL', 'FORNPREP',
           'FORNLOOP', 'FORGPREP', 'FORGLOOP', 'CLOSEUPVALS', 'GETVARARGS'):
    REG_FIELDS[_n] = ('A',)
for _n in ('MOVE', 'GETTABLEKS', 'SETTABLEKS', 'SELF', 'LENGTH', 'NOT', 'MINUS', 'GETTABLEN', 'SETTABLEN', 'ORK', 'ANDK', 'SETLIST') + _ARITH_K:
    REG_FIELDS[_n] = ('A', 'B')
for _n in ('ADD', 'SUB', 'MUL', 'DIV', 'MOD', 'POW', 'IDIV', 'AND', 'OR', 'GETTABLE', 'SETTABLE', 'CONCAT'):
    REG_FIELDS[_n] = ('A', 'B', 'C')
for _n in ('SUBRK', 'DIVRK'):
    REG_FIELDS[_n] = ('A', 'C')
for _n in ('JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT', 'JUMPIFNOTLT', 'JUMPIFNOTLE'):
    REG_FIELDS[_n] = ('A', 'aux')
for _n in ('JUMP', 'NOP'):
    REG_FIELDS[_n] = ()


def unshift_args(name, a, rs):
    """the operands of one instruction with the register offset taken off"""
    if name == 'CAPTURE':
        fields = ('B',) if a['A'] in (0, 1) else ()
    else:
        fields = REG_FIELDS.get(name)
        if fields is None:
            raise DecodeError('instruction %s has no register layout (needed for the register offset)' % name)
    if not fields:
        return a
    b = dict(a)
    for k in fields:
        b[k] = a[k] - rs
    return b


class DecodeError(Exception):
    pass


class Decoder(object):
    def __init__(self, dump, mapping, perm, fused_perm, junk_op):
        self.dump = dump
        self.protos = dump.protos
        self.map = mapping
        self.perm = perm
        self.fperm = fused_perm
        self.junk = junk_op
        self.xorkey = dump.xorkey

    # ------------------------------------------------------------------ fields
    def logical(self, df, T):
        pm = self.perm[df]
        d = dict(zip(RAWF, [T[i - 1] for i in pm]))
        return dict(op=d['op'], ef=d['ef'], A=d['A'], B=d['B'], C=d['C'], rf=d['RF'], Hf=d['HF'], aux=d['AUX'],
                    nxt=d['NXT'], alt=d['ALT'])

    def fused(self, df, part):
        r = [part[i - 1] for i in self.fperm[df]]
        return dict(A=r[0], B=r[1], C=r[2], rf=r[3], Hf=r[4], aux=r[5])

    @staticmethod
    def fixed_args(f, fixed):
        a = dict(A=f['A'], B=0, C=0, rf=0, Hf=0, aux=0)
        a.update(fixed)
        return a

    # ------------------------------------------------------------------ one record
    def decode(self, p, rid):
        d = self._decode(p, rid)
        rs = self.protos[p].get('rs', 0)
        return self.unshift(d, rs) if rs else d

    def unshift(self, d, rs):
        """the same record with the function's register offset taken off every register operand"""
        out = dict(d)
        out['ins'] = [(n, unshift_args(n, a, rs)) for n, a in d['ins']]
        c = d['ctrl']
        out['ctrl'] = None if c is None else (c[0], unshift_args(c[0], c[1], rs))
        if d.get('extra'):
            out['extra'] = [(vr, self.unshift(vd, rs)) for vr, vd in d['extra']]
        return out

    def _decode(self, p, rid):
        P = self.protos[p]
        rec = P['recs'][rid]
        df = P['df']
        L = self.logical(df, rec['T'])
        sem = rec['sem']
        key = sem + p * 256
        nxt, alt = L['nxt'], L['alt']
        ins = []
        ctrl = None
        parts = rec['parts']
        ff = [self.fused(df, pr) for pr in parts]
        M = self.map
        where = 'proto %d record %d' % (p, rid)
        if rec['ded'] != '-':
            ti = int(rec['ded'])
            k = key if ti == 6 else sem
            micro = M.ded.get((ti, k))
            if micro is None:
                raise DecodeError('%s: dedicated handler %d/%d is not understood' % (where, ti, k))
            kind = 'D%d_%d' % (ti, k)
            if isinstance(micro, dict):
                return self.decode_loop(micro['loop'], ff, rid, sem, nxt, kind, where)
            for name, part, fixed, is_ctrl in micro:
                if part >= len(ff):
                    raise DecodeError('%s: record has too few parts for handler %d/%d' % (where, ti, k))
                f = ff[part]
                if name == 'TAILCALL':
                    kind = 'TAILCALL'
                    ins.append(('CALL', dict(A=f['A'], B=f['B'], C=0, rf=0, Hf=0, aux=0)))
                    ctrl = ('RETURN', dict(A=f['A'], B=0, C=0, rf=0, Hf=0, aux=0))
                    continue
                args = self.fixed_args(f, fixed) if fixed else f
                if is_ctrl:
                    ctrl = (name, args)
                else:
                    ins.append(self.simple(name, args))
            return dict(kind=kind, sem=sem, ins=ins, ctrl=ctrl, nxt=nxt, alt=alt, rid=rid)
        if rec['pat'] is not None:
            pt = rec['pat']
            if len(pt) != len(parts):
                raise DecodeError('%s: pattern/part count mismatch' % where)
            for i, b in enumerate(pt):
                op = b ^ self.xorkey
                ins.append(self.simple_op(op, ff[i], where))
            return dict(kind='PAT', sem=sem, ins=ins, ctrl=None, nxt=nxt, alt=alt, rid=rid)
        op = L['op']
        f = dict(A=L['A'], B=L['B'], C=L['C'], rf=L['rf'], aux=L['aux'], Hf=L['Hf'])
        if op == self.junk:
            mix = rec['mix']
            if mix is None:
                raise DecodeError('%s: no mixing value dumped for the junk operation' % where)
            return dict(kind='JUNK', sem=sem, ins=[], ctrl=None, nxt=L['nxt'] ^ int(mix), alt=alt, rid=rid)
        name = M.hot.get(op) or M.ctrl.get(op)
        if name is not None:
            d = self.plain(name, f, sem, nxt, alt, rid)
            if name in ('NEWCLOSURE', 'DUPCLOSURE') and len(parts) >= 2:
                self.fused_captures(d, df, parts, rid, nxt)
            return d
        if op in M.simple:
            return dict(kind='SIMPLE', sem=sem, ins=[self.simple_op(op, f, where)], ctrl=None, nxt=nxt, alt=alt, rid=rid)
        raise DecodeError('%s: operation %d is not understood (build not supported)' % (where, op))

    def fused_captures(self, d, df, parts, rid, nxt):
        """Closure records of newer builds carry the capture descriptors themselves (every part after the first is one),
        where older builds put a record per capture behind the closure.  Both are handed on in the older form: the
        closure is followed by CAPTURE records."""
        caps = parts[1:]
        ids = [((q + 1) << 40) | rid for q in range(len(caps))]
        extra = []
        for q, part in enumerate(caps):
            L = self.logical(df, part)
            f = dict(A=L['A'], B=L['B'], C=0, rf=0, Hf=0, aux=0)
            extra.append((ids[q], dict(kind='CAPTURE', sem=d['sem'], ins=[('CAPTURE', f)], ctrl=None,
                                       nxt=(ids[q + 1] if q + 1 < len(caps) else nxt), alt=NONE, rid=ids[q])))
        d['nxt'] = ids[0]
        d['extra'] = extra

    def decode_all(self, p):
        """every record of a function, the extra records that expand fused loops included: {rid: record}"""
        D = {}
        for rid in self.protos[p]['recs']:
            d = self.decode(p, rid)
            D[rid] = d
            for vr, vd in d.get('extra', ()):
                D[vr] = vd
        return D

    def decode_loop(self, lp, ff, rid, sem, nxt, kind, where):
        """A record that runs a whole loop (see Classifier.classify_loop_ded).  It is expanded into the records a compiler
        would have produced: the iterator check (jumping to the loop control), the loop control, the body."""
        def one(m):
            name, part, fixed, is_ctrl = m
            if part >= len(ff):
                raise DecodeError('%s: record has too few parts for a fused loop' % where)
            f = ff[part]
            return name, (self.fixed_args(f, fixed) if fixed else f)

        def vid(i):
            return ((i + 1) << 40) | rid

        none = dict(A=0, B=0, C=0, rf=0, Hf=0, aux=0)
        pre = []
        for m in lp['pre']:
            name, a = one(m)
            if m[3]:
                if name != 'FORGPREP':
                    raise DecodeError('%s: unexpected jump in front of a fused loop' % where)
                pre.append(('FORGPREP', a))
            else:
                pre.append(self.simple(name, a))
        body = [self.simple(*one(m)) for m in lp['body']]
        if lp['head']:
            name, a = one(lp['head'][0])
            head, bod = vid(0), vid(1)
            entry = dict(kind=kind, sem=sem, ins=pre, ctrl=('JUMP', none), nxt=nxt, alt=head, rid=rid)
            extra = [(head, dict(kind=kind, sem=sem, ins=[], ctrl=(name, a), nxt=nxt, alt=(bod if body else head), rid=head))]
            if body:
                extra.append((bod, dict(kind=kind, sem=sem, ins=body, ctrl=('JUMP', none), nxt=nxt, alt=head, rid=bod)))
            entry['extra'] = extra
            return entry
        name, a = one(lp['tail'][0])
        if not pre:
            return dict(kind=kind, sem=sem, ins=body, ctrl=(name, a), nxt=nxt, alt=rid, rid=rid)
        loop = vid(0)
        entry = dict(kind=kind, sem=sem, ins=pre, ctrl=None, nxt=loop, alt=NONE, rid=rid)
        entry['extra'] = [(loop, dict(kind=kind, sem=sem, ins=body, ctrl=(name, a), nxt=nxt, alt=loop, rid=loop))]
        return entry

    def simple_op(self, op, f, where):
        m = self.map.simple.get(op)
        if m is None:
            raise DecodeError('%s: simple operation %d is not understood' % (where, op))
        if isinstance(m, (list, tuple)):
            names = m[1]
            n = names.get(f['Hf'], names.get(str(f['Hf'])))
            if n is None:
                raise DecodeError('%s: operation %d with selector %s is not understood' % (where, op, f['Hf']))
            m = n
        return self.simple(m, f)

    @staticmethod
    def simple(name, f):
        return (name, dict(A=f['A'], B=f['B'], C=f['C'], rf=f['rf'], Hf=f['Hf'], aux=f['aux']))

    def plain(self, name, f, sem, nxt, alt, rid):
        if name in CTRL_OPS:
            return dict(kind=name, sem=sem, ins=[], ctrl=(name, f), nxt=nxt, alt=alt, rid=rid)
        if name == 'JUMP':
            return dict(kind='JUMP', sem=sem, ins=[], ctrl=('JUMP', f), nxt=nxt, alt=alt, rid=rid)
        if name == 'FORGPREP':
            return dict(kind='FORGPREP', sem=sem, ins=[('FORGPREP', f)], ctrl=('JUMP', f), nxt=nxt, alt=alt, rid=rid)
        if name == 'LOADB':
            return dict(kind='LOADB', sem=sem, ins=[('LOADB', f)], ctrl=(('JUMP', f) if f['C'] != 0 else None),
                        nxt=nxt, alt=alt, rid=rid)
        if name in DATA_OPS or name == 'CAPTURE':
            return dict(kind=name, sem=sem, ins=[(name, f)], ctrl=None, nxt=nxt, alt=alt, rid=rid)
        if name == 'NOP':
            return dict(kind='NOP', sem=sem, ins=[], ctrl=None, nxt=nxt, alt=alt, rid=rid)
        return dict(kind='SIMPLE', sem=sem, ins=[(name, f)], ctrl=None, nxt=nxt, alt=alt, rid=rid)
