"""Run the engine in a separate, resource-limited process with a hard deadline.

This is the entry point for both the CLI and the Discord bot: a hostile or broken script can neither hang nor
exhaust the host, because the whole worker (and the Luau processes it starts) is killed when time is up."""
import json
import os
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time

from .engine import EngineError, Options, Result

WORKER = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'worker.py')


def _limits(mem_bytes):
    def apply():
        try:
            import resource
            if mem_bytes:
                resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        except Exception:
            pass
        try:
            os.setsid()
        except Exception:
            pass
    return apply


def _kill(proc):
    try:
        if hasattr(os, 'killpg'):
            os.killpg(proc.pid, 9)
        else:
            proc.kill()
    except Exception:
        try:
            proc.kill()
        except Exception:
            pass


def _pump(stream, q):
    try:
        for line in iter(stream.readline, b''):
            q.put(line)
    finally:
        q.put(None)


def run(data, opts=None, hard_timeout=240, mem_mb=3072, progress=None, cancel=None):
    """Deobfuscate `data` (bytes) in a worker process.  Returns Result; raises EngineError.
    `progress(stage)` is called from the calling thread; `cancel()` -> True aborts the job."""
    opts = opts or Options()
    note = progress or (lambda s: None)
    if not data or not data.strip():
        raise EngineError('empty', 'the file is empty')
    if len(data) > opts.max_bytes:
        raise EngineError('too_big', 'the file is larger than %d bytes' % opts.max_bytes)
    work = tempfile.mkdtemp(prefix='bgdeobf_job_')
    proc = None
    try:
        with open(os.path.join(work, 'in.lua'), 'wb') as f:
            f.write(data)
        with open(os.path.join(work, 'cfg.json'), 'w') as f:
            json.dump(dict(vars(opts)), f)
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin')}
        for k in ('BGDEOBF_LUAU_DIR', 'SYSTEMROOT', 'LANG', 'LC_ALL'):
            if os.environ.get(k):
                env[k] = os.environ[k]
        env['TMPDIR'] = env['TEMP'] = env['TMP'] = work
        try:
            proc = subprocess.Popen([sys.executable, '-I', WORKER, work], cwd=work,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                    env=env, preexec_fn=_limits(mem_mb * 1024 * 1024) if os.name == 'posix' else None)
        except OSError as e:
            raise EngineError('internal', 'cannot start the worker: %s' % e)
        q = queue.Queue()
        threading.Thread(target=_pump, args=(proc.stdout, q), daemon=True).start()
        err_buf = []
        threading.Thread(target=lambda: err_buf.append(proc.stderr.read()), daemon=True).start()
        deadline = time.time() + hard_timeout
        final = None
        while True:
            if cancel is not None and cancel():
                _kill(proc)
                raise EngineError('cancelled', 'the job was cancelled')
            left = deadline - time.time()
            if left <= 0:
                _kill(proc)
                raise EngineError('timeout', 'the job took longer than %d s and was stopped' % hard_timeout)
            try:
                line = q.get(timeout=min(0.25, left))
            except queue.Empty:
                continue
            if line is None:
                break
            try:
                msg = json.loads(line.decode('utf-8', 'replace'))
            except ValueError:
                continue
            if not isinstance(msg, dict):
                continue
            if 'stage' in msg:
                note(msg['stage'])
            elif 'ok' in msg:
                final = msg
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            _kill(proc)
        if final is None:
            rc = proc.returncode
            time.sleep(0.05)
            tail = (err_buf[0] if err_buf else b'').decode('utf-8', 'replace').strip()[-300:]
            raise EngineError('memory' if rc in (-9, -6, -11) else 'internal',
                              'the worker stopped unexpectedly (exit %s) %s' % (rc, tail))
        if not final['ok']:
            raise EngineError(final.get('code', 'internal'), final.get('message', 'failed'))
        with open(os.path.join(work, 'out.lua'), 'rb') as f:
            text = f.read().decode('utf-8')
        return Result(text, final['info'])
    finally:
        if proc is not None and proc.poll() is None:
            _kill(proc)
            try:
                proc.wait(timeout=5)
            except Exception:
                pass
        shutil.rmtree(work, ignore_errors=True)
