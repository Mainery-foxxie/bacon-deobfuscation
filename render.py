"""Render a decoded program to Luau source."""
from .printer import *


class RenderResult(object):
    def __init__(self, text, log, used_names):
        self.text = text
        self.log = log
        self.used_names = used_names


def render(decoder, namemap=None, root_pid=None, main_pid=None, annotate=False):
    dec = Decompiler(decoder)
    root_pid = root_pid if root_pid is not None else main_pid
    gen = Gen(dec)
    reserved = set()
    for pid, P in dec.protos.items():
        for rid, d in decoder.decode_all(pid).items():
            for name, ins in d['ins']:
                if name == 'GETIMPORT':
                    k = (ins['aux'] >> 20) & 1023
                    c = P['consts'][k]
                    reserved.add(c[1].decode('latin1'))
    pr = Printer(gen, reserved, namemap=namemap or {}, annotate=annotate)
    root = Fn(dec, root_pid, [Web() for _ in range(dec.protos[root_pid]['nup'])])
    gen.run(root)
    hoist(root)
    sc = Scope(None)
    lines = pr.block(root.body, sc, '')
    text = '\n'.join(lines) + '\n'
    log = list(gen.log) + list(getattr(dec, 'gen_log', []))
    return RenderResult(text, log, pr.used_keys)
