"""Turn a Bacon Guard loader into a script that dumps the decoded VM instead of running the program."""
import os

from .astutil import unparen, walk, lkey
from .loader import Loader, LayoutError, loc_of, const_num

ROLE_ALIASES = ['MK', 'D1', 'D2', 'D3', 'D4', 'D5', 'D6', 'P1', 'P2', 'XORKEY', 'SIMPLESET', 'MIX']


def _tpl():
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'dumper.lua.tpl'), encoding='latin1') as f:
        return f.read()


def tail_replacement(L):
    """The loader's last statement and the text that replaces it: a call of the dumper, after evaluating whatever builds
    the program.  Four shapes are known:
      return RUN(...)  with  local function RUN(...) return MODULE(host, env)(...) end   (the call moves into the dumper)
      return RUN(...)  with  local RUN = <anything else>                                  (the dumper is called as is)
      return MODULE(host, env)(...)                                                       (newer builds: the program is
                                                                                           stored in compressed pages)
      return R[1](...) with  local R = {MODULE(host, env)}                                (the same, with the results of
                                                                                           the module call in a table;
                                                                                           it has run before the tail)"""
    src = L.src
    last = L.root['body'][-1]
    if last['type'] != 'AstStatReturn' or len(last['list']) != 1:
        raise LayoutError('loader tail not recognised')
    call = unparen(last['list'][0])
    if call['type'] != 'AstExprCall':
        raise LayoutError('loader tail not recognised')
    outer = unparen(call['func'])
    if outer['type'] == 'AstExprCall' and loc_of(outer['func']) is not None:
        return last, '__DUMPER(' + src.slice(outer) + ')'
    if outer['type'] == 'AstExprIndexExpr' and loc_of(outer['expr']) is not None and const_num(outer['index']) is not None:
        decl = L.local_decl.get(lkey(loc_of(outer['expr'])))
        if decl is not None and decl[0]['type'] == 'AstStatLocal':
            return last, '__DUMPER()'
        raise LayoutError('loader runner not recognised')
    if loc_of(call['func']) is None:
        raise LayoutError('loader tail not recognised')
    key = lkey(loc_of(call['func']))
    fn = L.funcs.get(key)
    decl = L.local_decl.get(key)
    if fn is not None:
        st = fn['body']['body']
        if len(st) == 1 and st[0]['type'] == 'AstStatReturn':
            inner = unparen(st[0]['list'][0])
            if inner['type'] == 'AstExprCall' and unparen(inner['func'])['type'] == 'AstExprCall':
                return last, '__DUMPER(' + src.slice(unparen(inner['func'])) + ')'
    elif decl is not None and decl[0]['type'] == 'AstStatLocal':
        return last, '__DUMPER()'
    raise LayoutError('loader runner not recognised')


def build_dump_script(L):
    text = L.text
    src = L.src
    edits = []   # (offset, text) insertions

    # 1. early return after the per-proto readers exist
    prof_local = L.role_local['PROF']
    pst = None
    for s in L.frame_body:
        if s['type'] == 'AstStatLocal' and any(lkey(v) == lkey(prof_local) for v in s['vars']):
            pst = s
    if pst is None:
        raise LayoutError('profile statement not found')
    edits.append((src.span(pst)[0], 'if __DUMP~=nil and (...)==__DUMP then return end '))

    # 2. aliases of the internal tables / functions
    for role in ROLE_ALIASES:
        loc = L.role_local[role]
        decl = L.local_decl.get(lkey(loc))
        if decl is None:
            raise LayoutError('declaration of %s not found' % role)
        stmt = decl[0]
        edits.append((src.span(stmt)[1], ' __RL.%s=%s;' % (role, loc['name'])))
    # protos / main index: right after their use
    ms = L.main_stmt
    edits.append((src.span(ms)[1], ' __RL.PROTOS=%s;__RL.MAINIDX=%s;' % (L.role_local['PROTOS']['name'], L.role_local['MAINIDX']['name'])))

    # 3. replace the final call by the dumper
    last, tail_new = tail_replacement(L)
    a, b = src.span(last)

    # apply edits (descending offsets), then replace the tail
    out = text
    edits.sort(key=lambda e: -e[0])
    out = out[:a] + tail_new + out[b:]
    for off, ins in edits:
        if off > a:
            raise LayoutError('edit after tail')
        out = out[:off] + ins + out[off:]

    perm = '{' + ','.join('[%d]={%s}' % (k, ','.join(str(i) for i in v)) for k, v in sorted(L.perm.items())) + '}'
    head = _tpl().replace('%PERM%', perm).replace('%SEMIDX%', str(L.sem_index))
    return head + '\n' + out
