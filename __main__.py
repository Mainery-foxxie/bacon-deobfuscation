"""CLI:  python -m bgdeobf input.lua [-o output.lua]   (also several files or folders: python -m bgdeobf folder/ --jobs 4)"""
import argparse
import json
import os
import re
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

from .astutil import check_tools
from .engine import EngineError, Options, deobfuscate, detect, fill_trace, sandbox
from .messages import explain, verdict
from .namesfile import NamesError, parse_names
from .runner import run

EXTENSIONS = ('.lua', '.luau', '.txt')
# what this tool writes next to a script: results, the fake Roblox report and the texts it captured (foo_payload1.lua)
RESULT_NAME = re.compile(r'(_deobf\.lua|_annotated\.lua|_reconstructed\.lua|_trace\.txt|_payload\d+\.(lua|txt))$', re.I)


def expand_inputs(paths):
    """The files named on the command line plus the scripts directly inside every folder named (sorted, no sub-folders).
    What an earlier run wrote (`*_deobf.lua`, `*_annotated.lua`, `*_reconstructed.lua`, `*_trace.txt`, `*_payload<N>.lua|txt`) found
    inside a folder is not picked up again.
    Returns (files, problems)."""
    files, problems, seen = [], [], set()
    for p in paths:
        if os.path.isdir(p):
            found = []
            for n in sorted(os.listdir(p)):
                low = n.lower()
                if low.endswith(EXTENSIONS) and not RESULT_NAME.search(low) and os.path.isfile(os.path.join(p, n)):
                    found.append(os.path.join(p, n))
            if not found:
                problems.append('No .lua / .luau / .txt scripts found in %s' % p)
        else:
            found = [p]
        for f in found:
            key = os.path.abspath(f)
            if key not in seen:
                seen.add(key)
                files.append(f)
    return files, problems


def short_summary(info):
    """one line about a finished file, for batch mode"""
    f = info.get('fidelity')
    if f:
        txt = '%d/%d functions identical' % (f['identical'], f['functions'])
        extra = []
        if f.get('reordered'):
            extra.append('%d reordered' % f['reordered'])
        if f.get('cosmetic'):
            extra.append('%d cosmetic' % f['cosmetic'])
        if f.get('differing'):
            extra.append('%d differ' % len(f['differing']))
        return txt + (' (%s)' % ', '.join(extra) if extra else '')
    if info.get('syntax_ok'):
        return '%d functions, syntax ok' % info.get('protos', 0)
    return '%d functions' % info.get('protos', 0)


def main(argv=None):
    ap = argparse.ArgumentParser(prog='bgdeobf', description='Deobfuscate Roblox scripts protected with Bacon Guard / FORGE (MoonVeil: behaviour reconstruction only).')
    ap.add_argument('input', nargs='*', help='protected .lua / .luau / .txt file(s) or folder(s) (or - for stdin)')
    ap.add_argument('--check', action='store_true', help='check that the Luau tools are installed and can run')
    ap.add_argument('--detect', action='store_true',
                    help='only say whether each file is a Bacon Guard / FORGE (or MoonVeil) script and which build; nothing is run')
    ap.add_argument('-o', '--output', help='output file (default: <name>_deobf.lua, or <name>_annotated.lua with --annotate); '
                                           'with several files or a folder: the output folder (default: next to each input)')
    ap.add_argument('--jobs', type=int, default=1, help='several files or a folder: how many are processed at the same time (default 1)')
    ap.add_argument('--names', help='JSON file {key: name} used to name variables (keys are shown with --annotate)')
    ap.add_argument('--annotate', action='store_true', help='write each variable key into the output, to build a --names file')
    ap.add_argument('--no-verify', action='store_true', help='skip the syntax + bytecode checks')
    g = ap.add_argument_group('fake Roblox sandbox',
                              'Runs a script in a pretend Roblox (no network, no files, simulated time) and writes down what it does: '
                              'requests, remotes, windows, loadstring payloads, kicks, errors ...')
    g.add_argument('--trace', action='store_true', help='after deobfuscating, run the result in the fake Roblox; writes <name>_trace.txt')
    g.add_argument('--sandbox', action='store_true',
                   help='do not deobfuscate: run the script as it is in the fake Roblox; writes <name>_trace.txt')
    g.add_argument('--explore', action='store_true', help='afterwards press every button the script made, one after another')
    g.add_argument('--place-id', type=int, default=0, help='game.PlaceId in the fake Roblox (default 0)')
    g.add_argument('--user-id', type=int, help='UserId of the local player')
    g.add_argument('--username', help='name of the local player')
    g.add_argument('--executor', help='what identifyexecutor() answers (default Wave)')
    g.add_argument('--players', type=int, default=0, help='how many other players are in the server (default 0)')
    g.add_argument('--responses', help='JSON file {url: text}: what HttpGet / request answer; a url ending in * matches a prefix '
                                       '(default: an empty text for everything)')
    g.add_argument('--sandbox-time', type=float, default=30.0, help='simulated seconds to run (default 30)')
    g.add_argument('--sandbox-timeout', type=int, default=30, help='real seconds before the run is stopped (default 30)')
    ap.add_argument('--timeout', type=int, default=240, help='hard time limit per file, seconds (default 240)')
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
        ap.error('give a file name, a folder, or use --check')
    if a.jobs < 1:
        ap.error('--jobs must be at least 1')
    if '-' in a.input and len(a.input) > 1:
        ap.error('- (standard input) cannot be combined with other inputs')
    if a.jobs > 1 and a.in_process:
        ap.error('--jobs needs the separate worker processes; leave out --in-process')
    if a.trace and a.sandbox:
        ap.error('--trace and --sandbox cannot be combined (--sandbox runs the script as given, --trace runs the deobfuscated result)')
    if a.sandbox and (a.annotate or a.names):
        ap.error('--sandbox does not deobfuscate, so --annotate and --names do not apply')
    if not (a.trace or a.sandbox) and (a.explore or a.responses or a.user_id or a.username or a.executor or a.players or a.place_id):
        ap.error('--explore, --responses, --place-id, --user-id, --username, --executor and --players need --trace or --sandbox')
    responses = None
    if a.responses:
        try:
            with open(a.responses, 'rb') as f:
                responses = json.loads(f.read().decode('utf-8'))
            if not isinstance(responses, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in responses.items()):
                raise ValueError('it must be one JSON object of strings: {"https://...": "text"}')
        except (OSError, ValueError) as e:
            ap.error('invalid --responses file: %s' % e)
    trace_opts = dict(place_id=a.place_id, user_id=a.user_id, username=a.username, executor=a.executor, other_players=a.players,
                      responses=responses, explore=a.explore, max_time=a.sandbox_time, timeout=a.sandbox_timeout)

    if a.input == ['-']:
        files, problems = ['-'], []
    else:
        files, problems = expand_inputs(a.input)
    for p in problems:
        print(p, file=sys.stderr)
    if not files:
        return 1
    batch = len(files) > 1 or any(os.path.isdir(p) for p in a.input)
    if a.detect:
        return detect_files(files, a)

    names = None
    if a.names:
        try:
            with open(a.names, 'rb') as f:
                names = parse_names(f.read())
        except (OSError, NamesError) as e:
            print('Invalid --names file: %s' % e, file=sys.stderr)
            return 1
    opts = Options(max_bytes=int(a.max_mb * 1024 * 1024), verify=not a.no_verify, names=names, annotate=a.annotate,
                   trace=a.trace, sandbox_only=a.sandbox, trace_opts=trace_opts if (a.trace or a.sandbox) else None)

    out_dir = None
    if batch:
        if a.output:
            if os.path.exists(a.output) and not os.path.isdir(a.output):
                print('With several files or a folder, -o must be a folder (%s is a file)' % a.output, file=sys.stderr)
                return 1
            out_dir = a.output
    elif a.output and (os.path.isdir(a.output) or a.output.endswith(('/', os.sep))):
        out_dir = a.output
    if out_dir:
        try:
            os.makedirs(out_dir, exist_ok=True)
        except OSError as e:
            print('Cannot create the output folder: %s' % e, file=sys.stderr)
            return 1

    taken, lock = set(), threading.Lock()

    def output_path(inp, info):
        """-o when it names a file; otherwise <name>_deobf.lua (or _annotated, or _trace.txt with --sandbox) in the output
        folder or next to the input"""
        if a.output and not out_dir:
            return a.output
        stem = 'stdin' if inp == '-' else os.path.splitext(os.path.basename(inp))[0]
        suffix = ('_trace.txt' if info.get('sandbox_only') else '_reconstructed.lua' if info.get('trace_only')
                  else '_annotated.lua' if info.get('annotated') else '_deobf.lua')
        folder = out_dir or ('.' if inp == '-' else os.path.dirname(os.path.abspath(inp)))
        path, n = os.path.join(folder, stem + suffix), 2
        with lock:
            while path in taken:
                path = os.path.join(folder, '%s_%d%s' % (stem, n, suffix))
                n += 1
            taken.add(path)
        return path

    def process(inp, progress):
        """-> dict(file, ok, output, info | error, code, seconds); never raises for a problem with one file"""
        t0 = time.time()
        res_ = dict(file=inp, ok=False, output=None, info=None, error=None, code=None)
        try:
            data = sys.stdin.buffer.read() if inp == '-' else open(inp, 'rb').read()
        except OSError as e:
            res_.update(error='Cannot read the file: %s' % e, code='unreadable', unreadable=True)
            return res_
        try:
            if a.in_process:
                res = sandbox(data, opts, progress=progress) if a.sandbox else deobfuscate(data, opts, progress=progress)
            else:
                res = run(data, opts, hard_timeout=a.timeout, progress=progress)
        except EngineError as e:
            res_.update(error=explain(e.code, e.message), code=e.code)
            res_['seconds'] = time.time() - t0
            return res_
        except Exception as e:
            # one broken file must not stop a batch
            res_.update(error='%s: %s' % (type(e).__name__, str(e)[:200]), code='internal', seconds=time.time() - t0)
            return res_
        out = output_path(inp, res.info)
        extra = []
        try:
            if res.text is not None:
                with open(out, 'w', encoding='utf-8', newline='\n') as f:
                    f.write(res.text)
            if res.trace:
                extra = write_trace_files(res.trace, 'stdin' if inp == '-' else os.path.basename(inp), out, res.text is None)
        except OSError as e:
            res_.update(error='Cannot write the result: %s' % e, code='unwritable')
            res_['seconds'] = time.time() - t0
            return res_
        res_.update(ok=True, output=out, info=res.info, seconds=time.time() - t0, trace_files=extra)
        return res_

    t0 = time.time()
    if not batch:
        progress = None if a.quiet else (lambda s: print('  ... %s' % s, file=sys.stderr, flush=True))
        r = process(files[0], progress)
        if not r['ok']:
            if r.get('unreadable'):
                print(r['error'], file=sys.stderr)
                return 1
            print('FAILED: ' + r['error'], file=sys.stderr)
            return 2
        if a.json:
            print(json.dumps(r['info'], indent=1, default=str))
        elif not a.quiet:
            for line in verdict(r['info']):
                print(line)
            print('Done in %.1f s -> %s' % (time.time() - t0, r['output']))
            for path in r.get('trace_files') or []:
                if path != r['output']:
                    print('  also written: %s' % path)
        return 0

    # ---- several files or a folder
    results = []
    shown = [0]

    def report(r):
        with lock:
            shown[0] += 1
            n = '[%d/%d]' % (shown[0], len(files))
            if r['ok']:
                if not a.quiet and not a.json:
                    print('%s %s -> %s  %s  (%.1f s)' % (n, os.path.basename(r['file']), os.path.basename(r['output']),
                                                         short_summary(r['info']), r['seconds']))
                    for w in r['info'].get('warnings', []):
                        print('      Note: ' + w)
            else:
                print('%s FAILED: %s: %s' % (n, r['file'], r['error']), file=sys.stderr)

    if a.jobs == 1:
        for f in files:
            r = process(f, None)
            report(r)
            results.append(r)
    else:
        with ThreadPoolExecutor(max_workers=min(a.jobs, len(files))) as pool:
            futures = [pool.submit(process, f, None) for f in files]
            for fut in as_completed(futures):
                report(fut.result())
            results = [fu.result() for fu in futures]
    failed = [r for r in results if not r['ok']]
    if a.json:
        print(json.dumps([dict(file=r['file'], ok=r['ok'], output=r['output'], error=r['error'], code=r['code'], info=r['info'])
                          for r in results], indent=1, default=str))
    elif not a.quiet:
        print('Done: %d of %d files in %.1f s%s; results %s' % (
            len(results) - len(failed), len(results), time.time() - t0, ' (%d failed)' % len(failed) if failed else '',
            'are in ' + out_dir if out_dir else 'are next to each input file'))
    return 2 if failed else 0


def write_trace_files(trace, name, out, report_is_out):
    """The report and the captured texts next to the output file `out`: foo_deobf.lua (or foo_reconstructed.lua) -> foo_trace.txt,
    foo_payload1.lua ...
    (`report_is_out`: with --sandbox `out` is the report itself).  Returns the paths written."""
    base = out
    for suffix in ('_deobf.lua', '_annotated.lua', '_reconstructed.lua', '_trace.txt'):
        if base.endswith(suffix):
            base = base[:-len(suffix)]
            break
    else:
        base = os.path.splitext(base)[0]
    report, files = fill_trace(trace, name, os.path.basename(base))
    report_path = out if report_is_out else base + '_trace.txt'
    written = []
    with open(report_path, 'w', encoding='utf-8', newline='\n') as f:
        f.write(report)
    written.append(report_path)
    for fname, data in files:
        path = os.path.join(os.path.dirname(base), fname)
        with open(path, 'wb') as f:
            f.write(data)
        written.append(path)
    return written


def detect_files(files, a):
    """--detect: one line per file `name<TAB>kind<TAB>confidence<TAB>label` (kind: bacon_guard | moonveil | unknown).
    Exit code 0 when every file is a supported Bacon Guard / FORGE layout or a MoonVeil script (behaviour trace only), 2 when not."""
    rows, bad = [], 0
    for inp in files:
        try:
            data = sys.stdin.buffer.read() if inp == '-' else open(inp, 'rb').read()
        except OSError as e:
            print('Cannot read the file: %s' % e, file=sys.stderr)
            bad += 1
            continue
        try:
            d = detect(data, max_bytes=int(a.max_mb * 1024 * 1024))
        except EngineError as e:
            print('FAILED: ' + explain(e.code, e.message), file=sys.stderr)
            bad += 1
            continue
        d['file'] = inp
        rows.append(d)
        if not d['supported']:
            bad += 1
    if a.json:
        print(json.dumps(rows, indent=1, default=str))
    else:
        for d in rows:
            label = d['label'] if d['supported'] or not d['detail'] else '%s: %s' % (d['label'], d['detail'])
            print('%s\t%s\t%.2f\t%s' % (os.path.basename(d['file']), d['kind'], d['confidence'], label))
    return 2 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
