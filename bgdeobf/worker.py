"""Worker process: python -I worker.py <workdir>.  Reads <workdir>/cfg.json + in.lua, writes <workdir>/out.lua.
Talks to the parent with one JSON object per line on stdout."""
import json
import os
import sys


def emit(obj):
    sys.stdout.write(json.dumps(obj) + '\n')
    sys.stdout.flush()


def main(argv):
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    from bgdeobf.engine import EngineError, Options, deobfuscate
    workdir = argv[1]
    try:
        with open(os.path.join(workdir, 'cfg.json')) as f:
            cfg = json.load(f)
        with open(os.path.join(workdir, 'in.lua'), 'rb') as f:
            data = f.read()
        opts = Options(**cfg)
        res = deobfuscate(data, opts, progress=lambda s: emit({'stage': s}))
        with open(os.path.join(workdir, 'out.lua'), 'w', encoding='utf-8', newline='\n') as f:
            f.write(res.text)
        emit({'ok': True, 'info': res.info})
    except EngineError as e:
        emit({'ok': False, 'code': e.code, 'message': e.message})
    except MemoryError:
        emit({'ok': False, 'code': 'memory', 'message': 'ran out of memory'})
    except RecursionError:
        emit({'ok': False, 'code': 'decompile', 'message': 'the program is too deeply nested'})
    except Exception as e:
        emit({'ok': False, 'code': 'internal', 'message': '%s: %s' % (type(e).__name__, str(e)[:200])})


if __name__ == '__main__':
    main(sys.argv)
