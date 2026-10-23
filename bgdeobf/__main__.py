"""CLI:  python -m bgdeobf input.lua [-o output.lua]"""
import argparse
import json
import os
import sys
import time

from .astutil import check_tools
from .engine import EngineError, Options, deobfuscate
from .messages import explain, verdict
from .namesfile import NamesError, parse_names
from .runner import run


def main(argv=None):
    ap = argparse.ArgumentParser(prog='bgdeobf', description='Deobfuscate Roblox scripts protected with Bacon Guard / FORGE.')
    ap.add_argument('input', nargs='?', help='protected .lua / .txt file (or - for stdin)')
    ap.add_argument('--check', action='store_true', help='check that the Luau tools are installed and can run')
    ap.add_argument('-o', '--output', help='output file (default: <name>_deobf.lua, or <name>_annotated.lua with --annotate)')
    ap.add_argument('--names', help='JSON file {key: name} used to name variables (keys are shown with --annotate)')
    ap.add_argument('--annotate', action='store_true', help='write each variable key into the output, to build a --names file')
    ap.add_argument('--no-verify', action='store_true', help='skip the syntax + bytecode checks')
    ap.add_argument('--timeout', type=int, default=240, help='hard time limit, seconds (default 240)')
    ap.add_argument('--max-mb', type=float, default=3.0, help='maximum file size, MB (default 3)')
    ap.add_argument('--in-process', action='store_true', help='run without a separate process (debugging, no limits)')
    ap.add_argument('--json', action='store_true', help='print the info as JSON')
    ap.add_argument('-q', '--quiet', action='store_true')
    a = ap.parse_args(argv)
    if a.check:
        bad = 0
        for name, ok, msg in check_tools():
            print('%-13s %s  %s' % (name, 'OK  ' if ok else 'FAIL', msg))
            bad += 0 if ok else 1
        return 1 if bad else 0
    if not a.input:
        ap.error('give a file name, or use --check')

    try:
        data = sys.stdin.buffer.read() if a.input == '-' else open(a.input, 'rb').read()
    except OSError as e:
        print('Cannot read the file: %s' % e, file=sys.stderr)
        return 1
    names = None
    if a.names:
        try:
            with open(a.names, 'rb') as f:
                names = parse_names(f.read())
        except (OSError, NamesError) as e:
            print('Invalid --names file: %s' % e, file=sys.stderr)
            return 1
    opts = Options(max_bytes=int(a.max_mb * 1024 * 1024), verify=not a.no_verify, names=names, annotate=a.annotate)
    t0 = time.time()
    progress = None if a.quiet else (lambda s: print('  ... %s' % s, file=sys.stderr, flush=True))
    try:
        if a.in_process:
            res = deobfuscate(data, opts, progress=progress)
        else:
            res = run(data, opts, hard_timeout=a.timeout, progress=progress)
    except EngineError as e:
        print('FAILED: ' + explain(e.code, e.message), file=sys.stderr)
        return 2
    out = a.output
    if not out:
        stem = 'stdin' if a.input == '-' else os.path.splitext(os.path.basename(a.input))[0]
        suffix = '_annotated.lua' if res.info.get('annotated') else '_deobf.lua'
        out = os.path.join('.' if a.input == '-' else os.path.dirname(os.path.abspath(a.input)), stem + suffix)
    with open(out, 'w', encoding='utf-8', newline='\n') as f:
        f.write(res.text)
    if a.json:
        print(json.dumps(res.info, indent=1, default=str))
    elif not a.quiet:
        for line in verdict(res.info):
            print(line)
        print('Done in %.1f s -> %s' % (time.time() - t0, out))
    return 0


if __name__ == '__main__':
    sys.exit(main())
