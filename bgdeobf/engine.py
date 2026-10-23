"""End-to-end pipeline: Bacon Guard / FORGE protected script -> readable Luau source."""
import os
import re
import subprocess
import tempfile
import time

from . import fidelity
from .astutil import LuauError, LuauMissing, LuauSyntax, LuauTimeout, find_bin, normalize_source
from .classify import ClassifyError, Classifier, collect_domains, RAWF
from .decode import DecodeError, Decoder
from .dump import DumpError, parse_dump
from .loader import LayoutError, Loader, extend
from .messages import scrub
from .patch import build_dump_script
from .render import render
from .sandbox import SandboxError, run_luau

BANNER = re.compile(r'Bacon Guard v([0-9.]+)\s*\|\s*([^|\n]*?)\s*\|\s*Build\s+([0-9a-fA-F]+)')


class EngineError(Exception):
    """code: empty | too_big | binary | syntax | unsupported | setup | timeout | sandbox | memory | decode | decompile | internal"""

    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


class Options(object):
    def __init__(self, max_bytes=3 * 1024 * 1024, sandbox_timeout=60, sandbox_mem_mb=1024, classify_timeout=120,
                 verify=True, names=None, annotate=False):
        self.max_bytes = max_bytes
        self.sandbox_timeout = sandbox_timeout
        self.sandbox_mem_mb = sandbox_mem_mb
        self.classify_timeout = classify_timeout
        self.verify = verify
        self.names = names
        self.annotate = annotate


class Result(object):
    def __init__(self, text, info):
        self.text = text
        self.info = info


def _check_perms(L):
    """the record layouts found in the loader must be well formed; returns a list of soft remarks"""
    remarks = []
    if not L.perm:
        raise LayoutError('no record layout found')
    for df, pm in sorted(L.perm.items()):
        fp = L.fused_perm.get(df)
        sp = L.simple_perm.get(df)
        if len(pm) != 10 or sorted(pm) != list(range(1, 11)):
            raise LayoutError('record layout %s is not a permutation of 10 fields' % df)
        if fp is None or sp is None:
            raise LayoutError('record layout %s has no fused/simple reader' % df)
        if len(fp) != 6 or len(sp) != 7 or len(set(fp)) != 6 or len(set(sp)) != 7 \
                or min(fp + sp) < 1 or max(fp + sp) > 10:
            raise LayoutError('record layout %s has a malformed fused/simple reader' % df)
        if fp != [pm[i] for i in (2, 3, 4, 5, 6, 7)] or sp != [pm[i] for i in (0, 2, 3, 4, 5, 6, 7)]:
            remarks.append('record layout %s differs from the known pattern' % df)
    return remarks


def _coverage(dump, L, mapping):
    """handlers used by the program that were not understood -> list of messages"""
    problems = []
    used_plain, used_simple, used_ded = set(), set(), set()
    for idx, P in dump.protos.items():
        pm = L.perm[P['df']]
        for rid, rec in P['recs'].items():
            if rec['ded'] != '-':
                ti = int(rec['ded'])
                used_ded.add((ti, rec['sem'] + idx * 256 if ti == 6 else rec['sem']))
            elif rec['pat'] is not None:
                for b in rec['pat']:
                    used_simple.add(b ^ dump.xorkey)
            else:
                used_plain.add(rec['T'][pm[0] - 1])
    for op in sorted(used_plain):
        if op == L.junk_op or op in mapping.hot or op in mapping.ctrl or op in mapping.simple:
            continue
        problems.append('operation %d' % op)
    for op in sorted(used_simple):
        if op not in mapping.simple:
            problems.append('operation %d (pattern)' % op)
    for k in sorted(used_ded):
        if k not in mapping.ded:
            problems.append('dedicated handler %d/%d' % k)
    return problems


def _tidy_errors(stderr, path, limit=3):
    """compiler output -> one short line per error (`line 8:14: SyntaxError: ...`), without the host's temp path"""
    lines = [ln.strip() for ln in scrub(stderr, path).splitlines() if ln.strip()]
    msg = '; '.join(ln[:200] for ln in lines[:limit])
    if len(lines) > limit:
        msg += ' (+%d more)' % (len(lines) - limit)
    return msg or 'unknown error'


def compile_check(text, timeout=60):
    """(ok, message, bytecode_text) from luau-compile; the message never contains host paths"""
    exe = find_bin('luau-compile')
    fd, path = tempfile.mkstemp(suffix='.lua')
    try:
        with os.fdopen(fd, 'wb') as f:
            f.write(text.encode('utf-8'))
        r = subprocess.run([exe, '--text', '-O1', '-g0', '--dump-constants', path], capture_output=True, timeout=timeout)
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass
    if r.returncode != 0:
        return False, _tidy_errors(r.stderr.decode('latin1', 'replace'), path), None
    return True, '', r.stdout.decode('latin1')


def deobfuscate(data, opts=None, progress=None):
    opts = opts or Options()
    note = progress or (lambda s: None)
    t0 = time.time()
    timings = {}
    info = {'warnings': []}

    def stage(name):
        note(name)
        return time.time()

    if not data or not data.strip():
        raise EngineError('empty', 'the file is empty')
    if len(data) > opts.max_bytes:
        raise EngineError('too_big', 'the file is larger than %d bytes' % opts.max_bytes)
    if b'\x00' in data[:4096]:
        raise EngineError('binary', 'the file does not look like a text script')
    text = normalize_source(data)
    m = BANNER.search(text[:4000])
    if m:
        info['banner'] = dict(version=m.group(1), protection=m.group(2), build=m.group(3))

    # ---- 1. find the pieces of the VM in the loader
    t = stage('analyse')
    try:
        L = Loader(text)
        extend(L)
        info['warnings'].extend(_check_perms(L))
    except LayoutError as e:
        raise EngineError('unsupported', 'not a supported Bacon Guard layout: %s' % e)
    except LuauSyntax as e:
        raise EngineError('syntax', 'not valid Luau: %s' % e)
    except LuauMissing as e:
        raise EngineError('setup', str(e))
    except LuauTimeout as e:
        raise EngineError('timeout', str(e))
    except LuauError as e:
        raise EngineError('unsupported', 'the script could not be analysed: %s' % e)
    except RecursionError:
        raise EngineError('unsupported', 'the script is too deeply nested to analyse')
    except (KeyError, IndexError, TypeError, AssertionError, AttributeError, ValueError) as e:
        raise EngineError('unsupported', 'unexpected loader structure (%s)' % type(e).__name__)
    timings['analyse'] = time.time() - t

    # ---- 2. unpack the VM image in a sandbox
    t = stage('unpack')
    try:
        script = build_dump_script(L)
    except LayoutError as e:
        raise EngineError('unsupported', 'loader entry points not recognised: %s' % e)
    try:
        out = run_luau(script, timeout=opts.sandbox_timeout, mem_mb=opts.sandbox_mem_mb)
    except SandboxError as e:
        raise EngineError('timeout' if e.code == 'timeout' else 'sandbox', e.message)
    except LuauMissing as e:
        raise EngineError('setup', str(e))
    except LuauError as e:
        raise EngineError('sandbox', str(e))
    try:
        dump = parse_dump(out.decode('latin1'))
    except (DumpError, ValueError, KeyError) as e:
        raise EngineError('sandbox', 'the unpacked image could not be read (%s)' % e)
    timings['unpack'] = time.time() - t
    for P in dump.protos.values():
        if P['df'] not in L.perm:
            raise EngineError('unsupported', 'a function uses record layout %s that the loader does not define' % P['df'])
    info['protos'] = len(dump.protos)
    info['records'] = sum(len(p['recs']) for p in dump.protos.values())

    # ---- 3. understand the handlers of this build
    t = stage('classify')
    try:
        mapping = Classifier(L, collect_domains(L, dump), deadline=time.time() + opts.classify_timeout).run(dump.simple)
    except ClassifyError as e:
        raise EngineError('timeout', str(e))
    except (KeyError, IndexError, TypeError, AttributeError) as e:
        raise EngineError('unsupported', 'a handler of this build could not be analysed (%s)' % type(e).__name__)
    timings['classify'] = time.time() - t
    missing = _coverage(dump, L, mapping)
    if missing:
        raise EngineError('unsupported', 'this build uses handlers that are not understood yet: ' + ', '.join(missing[:8]))
    info['handlers'] = dict(simple=len(mapping.simple), hot=len(mapping.hot), ctrl=len(mapping.ctrl), ded=len(mapping.ded))

    # ---- 4. decode + decompile
    t = stage('decompile')
    dec = Decoder(dump, mapping, L.perm, L.fused_perm, L.junk_op)
    try:
        res = render(dec, namemap=opts.names, main_pid=dump.main, annotate=opts.annotate)
    except DecodeError as e:
        raise EngineError('decode', str(e))
    except RecursionError:
        raise EngineError('decompile', 'the program is too deeply nested for the decompiler')
    except Exception as e:
        raise EngineError('decompile', 'internal decompiler error: %s: %s' % (type(e).__name__, str(e)[:200]))
    timings['decompile'] = time.time() - t
    text_out = res.text
    info['lines'] = text_out.count('\n')
    info['duplicated_blocks'] = len(res.log)

    # ---- 5. verification (an annotated copy has `<@key>` markers and is not valid Luau, so a clean copy is checked)
    if opts.annotate:
        info['annotated'] = True
    if opts.verify:
        t = stage('verify')
        check_text = text_out
        if opts.annotate:
            try:
                check_text = render(dec, namemap=opts.names, main_pid=dump.main, annotate=False).text
            except Exception:
                check_text = None
        try:
            if check_text is None:
                ok, msg, bc = None, 'the result could not be verified (no clean copy to check)', None
            else:
                ok, msg, bc = compile_check(check_text)
        except (LuauError, subprocess.TimeoutExpired, OSError) as e:
            ok, msg, bc = None, 'compile check unavailable: %s' % e, None
        if ok is False:
            info['warnings'].append('the result has a syntax error: ' + msg)
        elif ok:
            info['syntax_ok'] = True
            fd, bcpath = tempfile.mkstemp(suffix='.bc.txt')
            try:
                with os.fdopen(fd, 'w', encoding='latin1') as f:
                    f.write(bc)
                try:
                    rep = fidelity.compare(dec, bcpath)
                    cos = rep.get('cosmetic', [])
                    info['fidelity'] = dict(functions=rep['functions_orig'], identical=rep['identical'],
                                            reordered=rep['reordered'], cosmetic=len(cos),
                                            differing=[p for p in rep['differing'] if p not in cos])
                except Exception as e:
                    info['warnings'].append('fidelity check failed: %s' % type(e).__name__)
            finally:
                try:
                    os.unlink(bcpath)
                except OSError:
                    pass
        elif ok is None:
            info['warnings'].append(msg)
        timings['verify'] = time.time() - t
    timings['total'] = time.time() - t0
    info['timings'] = dict((k, round(v, 2)) for k, v in timings.items())
    return Result(text_out, info)
