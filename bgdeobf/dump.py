"""Parser for the text dump produced by the patched loader (see patch.py / dumper.lua.tpl)."""
import re


class DumpError(Exception):
    pass


class Dump(object):
    def __init__(self):
        self.protos = {}
        self.ded = {}
        self.pat1 = {}
        self.pat2 = {}
        self.simple = set()
        self.xorkey = None
        self.main = None


_R_RE = re.compile(r'(\d+) (\d+) sem=(\d+) ded=(\S+) pat=(\S+) raw=(\S+) parts=(\S*)(?: mix=(\S+))?$')


def parse_dump(text):
    d = Dump()
    for line in text.split('\n'):
        line = line.rstrip('\r')
        if not line:
            continue
        tag, _, rest = line.partition(' ')
        if tag == 'N':
            kv = rest.split(' ')
            try:
                d.xorkey = int(kv[kv.index('xorkey') + 1])
                d.main = int(kv[kv.index('main') + 1])
            except (ValueError, IndexError):
                raise DumpError('bad header line')
        elif tag == 'D':
            ti, _, keys = rest.partition(' ')
            d.ded[int(ti)] = set(int(k) for k in keys.split(',') if k != '')
        elif tag == 'P1':
            k, h = rest.split(' ')
            d.pat1[int(k)] = bytes.fromhex(h)
        elif tag == 'P2':
            k, h = rest.split(' ')
            d.pat2[int(k)] = bytes.fromhex(h)
        elif tag == 'S':
            d.simple = set(int(k) for k in rest.split(',') if k != '')
        elif tag == 'P':
            parts = rest.split(' ')
            idx = int(parts[0])
            kv = dict(p.split('=') for p in parts[1:])
            d.protos[idx] = dict(idx=idx, np=int(kv['np']), va=int(kv['va']), nup=int(kv['nup']), ms=int(kv['ms']),
                                 entry=int(kv['entry']), df=int(kv['df']), vf=int(kv['vf']), nrec=int(kv['nrec']),
                                 kids=[], consts={}, recs={})
        elif tag == 'H':
            idx, _, kids = rest.partition(' ')
            d.protos[int(idx)]['kids'] = [int(k) for k in kids.split(',') if k != '']
        elif tag == 'C':
            idx, cid, ty, *val = rest.split(' ')
            v = val[0] if val else None
            if ty == 's':
                cv = ('s', bytes.fromhex(v) if v else b'')
            elif ty == 'n':
                cv = ('n', float(v))
            elif ty == 'z':
                cv = ('z', None)
            elif ty == 'b':
                cv = ('b', v == '1')
            elif ty == 'i':
                cv = ('i', [int(x) for x in v.split(',')] if v else [])
            elif ty == 't':
                cv = ('t', [tuple(int(y) for y in x.split(':')) for x in v.split(',')] if v else [])
            else:
                cv = ('?', v)
            d.protos[int(idx)]['consts'][int(cid)] = cv
        elif tag == 'R':
            m = _R_RE.match(rest)
            if m is None:
                raise DumpError('bad record line: ' + line[:80])
            idx, rid, sem, dd, pat, raw, parts, mix = m.groups()
            T = [int(x) if x != '-' else None for x in raw.split(',')]
            P = []
            if parts:
                for p in parts.split(';'):
                    P.append([int(x) if x != '-' else None for x in p.split(',')])
            d.protos[int(idx)]['recs'][int(rid)] = dict(sem=int(sem), T=T, parts=P, ded=dd,
                                                        pat=(bytes.fromhex(pat) if pat != '-' else None),
                                                        mix=(int(mix) if mix not in (None, '-') else None))
        else:
            raise DumpError('unknown dump line: ' + line[:60])
    if d.xorkey is None or not d.protos:
        raise DumpError('empty dump')
    return d
