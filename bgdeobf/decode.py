"""Instruction decoder: turns dumped VM records into Luau-bytecode style instructions using a classified handler map."""
from .classify import RAWF

NONE = 4294967295

# operations that end a record with a conditional / loop transfer (the node carries `nxt` and `alt`)
CTRL_OPS = ('RETURN', 'FORNLOOP', 'FORGLOOP', 'JUMPIFNOT', 'JUMPIF', 'JUMPIFEQ', 'JUMPIFNOTEQ', 'JUMPIFLE', 'JUMPIFLT',
            'JUMPIFNOTLT', 'JUMPIFNOTLE', 'JUMPXEQKB', 'JUMPXEQK', 'JUMPXEQKNIL', 'FORNPREP')
DATA_OPS = ('CALL', 'SETLIST', 'NEWCLOSURE', 'DUPCLOSURE', 'CLOSEUPVALS')


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
            return self.plain(name, f, sem, nxt, alt, rid)
        if op in M.simple:
            return dict(kind='SIMPLE', sem=sem, ins=[self.simple_op(op, f, where)], ctrl=None, nxt=nxt, alt=alt, rid=rid)
        raise DecodeError('%s: operation %d is not understood (build not supported)' % (where, op))

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
