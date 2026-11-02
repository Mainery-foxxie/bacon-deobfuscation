"""Run Luau code in a resource-limited child process: the patched loader that dumps the VM (run_luau) and the fake Roblox
sandbox of tracer.py (run_luau_live, which hands back what the script printed even when it had to be stopped)."""
import os
import re
import subprocess
import tempfile
import threading
import time

from .astutil import find_bin
from .messages import scrub


class SandboxError(Exception):
    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


def _limits(mem_bytes, cpu_seconds, fsize_bytes, new_session=False, nofile=None):
    def apply():
        try:
            import resource
            if mem_bytes:
                resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
            if cpu_seconds:
                resource.setrlimit(resource.RLIMIT_CPU, (cpu_seconds, cpu_seconds + 1))
            if fsize_bytes:
                resource.setrlimit(resource.RLIMIT_FSIZE, (fsize_bytes, fsize_bytes))
            if nofile:
                resource.setrlimit(resource.RLIMIT_NOFILE, (nofile, nofile))
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
        except Exception:
            pass
        if new_session:
            try:
                os.setsid()
            except Exception:
                pass
    return apply


def run_luau(text, timeout=60, mem_mb=1024, max_output_mb=128):
    """execute Luau source (latin1 text); returns stdout bytes.  Raises SandboxError."""
    exe = find_bin('luau')
    tmp = tempfile.mkdtemp(prefix='bgdeobf_')
    script = os.path.join(tmp, 'dump.lua')
    outp = os.path.join(tmp, 'out.txt')
    try:
        with open(script, 'wb') as f:
            f.write(text.encode('latin1'))
        with open(outp, 'wb') as out:
            try:
                if os.name == 'posix':
                    extra = dict(env={'PATH': '/usr/bin:/bin'},
                                 preexec_fn=_limits(mem_mb * 1024 * 1024, int(timeout) + 2, max_output_mb * 1024 * 1024))
                else:
                    extra = dict(env={'SYSTEMROOT': os.environ.get('SYSTEMROOT', '')})
                p = subprocess.Popen([exe, script], cwd=tmp, stdin=subprocess.DEVNULL, stdout=out,
                                     stderr=subprocess.PIPE, **extra)
            except OSError as e:
                raise SandboxError('sandbox', 'cannot start the Luau runtime: %s' % scrub(e))
            try:
                _, err = p.communicate(timeout=timeout)
            except subprocess.TimeoutExpired:
                p.kill()
                p.communicate()
                raise SandboxError('timeout', 'the protector took longer than %d s to unpack' % timeout)
        err = (err or b'').decode('latin1', 'replace').strip()
        size = os.path.getsize(outp)
        if p.returncode != 0:
            if size >= max_output_mb * 1024 * 1024:
                raise SandboxError('sandbox', 'the unpacked dump exceeded the output limit')
            raise SandboxError('sandbox', 'unpacking failed (exit %s): %s' % (p.returncode, scrub(err, script)[:300]))
        with open(outp, 'rb') as f:
            return f.read()
    finally:
        for n in (script, outp):
            try:
                os.unlink(n)
            except OSError:
                pass
        try:
            os.rmdir(tmp)
        except OSError:
            pass


# ---------------------------------------------------------------------------------------------------------------------
# the fake Roblox sandbox: the Luau runtime of tracer.py runs here and its log is read while it runs

_LIVE = []


def live_supported():
    """Can the output of the child be read line by line (a pseudo terminal)?  Luau writes to a pipe or a file in blocks, and
    a process that is killed loses its last block; on a terminal every line is written at once.  Without one (Windows, or
    a host without /dev/ptmx) the runtime pads its output instead (CONFIG.pad), which is slower and bigger."""
    if not _LIVE:
        ok = False
        if os.name == 'posix':
            try:
                import pty
                m, sl = pty.openpty()
                os.close(m)
                os.close(sl)
                ok = True
            except Exception:
                ok = False
        _LIVE.append(ok)
    return _LIVE[0]


class LiveRun(object):
    """what a run left behind.  status: ok | timeout | output | killed | crashed"""

    def __init__(self, out, err, status, code, seconds):
        self.out = out
        self.err = err
        self.status = status
        self.code = code
        self.seconds = seconds


def _kill_group(proc):
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


def run_luau_live(text, timeout=30, mem_mb=512, max_output_mb=64, live=None):
    """Execute Luau source (bytes) in a fresh empty folder and return a LiveRun.  The child gets no standard input, an empty
    environment, a memory limit, a CPU limit, a limit on open files, and its own process group; it is killed when the time or
    the output limit is over, and then everything it printed up to that moment is still returned (`live`: read it line by
    line, see live_supported)."""
    if isinstance(text, str):
        text = text.encode('latin1')
    if live is None:
        live = live_supported()
    exe = find_bin('luau')
    tmp = tempfile.mkdtemp(prefix='bgdeobf_trace_')
    script = os.path.join(tmp, 'sandbox.lua')
    outp = os.path.join(tmp, 'out.bin')
    max_bytes = int(max_output_mb * 1024 * 1024)
    master = None
    out_file = None
    proc = None
    chunks = []
    status = None
    t0 = time.time()
    try:
        with open(script, 'wb') as f:
            f.write(text)
        if os.name == 'posix':
            extra = dict(env={'PATH': '/usr/bin:/bin'},
                         preexec_fn=_limits(mem_mb * 1024 * 1024, int(timeout) + 2, max_bytes, new_session=True, nofile=32))
        else:
            extra = dict(env={'SYSTEMROOT': os.environ.get('SYSTEMROOT', '')})
        if live:
            import pty
            import tty
            master, slave = pty.openpty()
            try:
                tty.setraw(slave)       # nothing is translated or interpreted on the way out
            except Exception:
                pass
            stdout = slave
        else:
            slave = None
            out_file = open(outp, 'wb')
            stdout = out_file
        try:
            proc = subprocess.Popen([exe, 'sandbox.lua'], cwd=tmp, stdin=subprocess.DEVNULL, stdout=stdout,
                                    stderr=subprocess.PIPE, close_fds=True, **extra)
        except OSError as e:
            raise SandboxError('sandbox', 'cannot start the Luau runtime: %s' % scrub(e))
        finally:
            if slave is not None:
                os.close(slave)
            if out_file is not None:
                out_file.close()
        err_buf = bytearray()

        def drain():
            try:
                while True:
                    d = proc.stderr.read(4096)
                    if not d:
                        break
                    if len(err_buf) < 65536:
                        err_buf.extend(d)
            except Exception:
                pass
        th = threading.Thread(target=drain, daemon=True)
        th.start()
        deadline = t0 + timeout
        total = 0
        if master is not None:
            import select
            while True:
                left = deadline - time.time()
                if left <= 0:
                    status = 'timeout'
                    break
                try:
                    ready, _, _ = select.select([master], [], [], min(0.1, left))
                except (OSError, ValueError):
                    break
                if ready:
                    try:
                        d = os.read(master, 65536)
                    except OSError:
                        d = b''
                    if not d:
                        break
                    chunks.append(d)
                    total += len(d)
                    if total > max_bytes:
                        status = 'output'
                        break
                elif proc.poll() is not None:
                    break
            if status:
                _kill_group(proc)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                _kill_group(proc)
                proc.wait()
            # whatever is still in the terminal
            end = time.time() + 1.0
            while time.time() < end and total <= max_bytes:
                try:
                    ready, _, _ = select.select([master], [], [], 0.05)
                except (OSError, ValueError):
                    break
                if not ready:
                    break
                try:
                    d = os.read(master, 65536)
                except OSError:
                    break
                if not d:
                    break
                chunks.append(d)
                total += len(d)
            data = b''.join(chunks)
        else:
            while True:
                if proc.poll() is not None:
                    break
                if time.time() >= deadline:
                    status = 'timeout'
                    break
                try:
                    if os.path.getsize(outp) > max_bytes:
                        status = 'output'
                        break
                except OSError:
                    pass
                time.sleep(0.05)
            if status:
                _kill_group(proc)
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                _kill_group(proc)
                proc.wait()
            with open(outp, 'rb') as f:
                data = f.read(max_bytes + 1)
        th.join(timeout=1)
        if len(data) > max_bytes:
            data = data[:max_bytes]
            status = status or 'output'
        err = bytes(err_buf).decode('latin1', 'replace').strip()
        err = re.sub(r'(?:\./)?sandbox\.lua:(\d+):\s*', r'runtime line \1: ', err)       # the runtime's own file
        code = proc.returncode
        if status is None:
            if code == 0:
                status = 'ok'
            elif code is not None and code < 0:
                status = 'killed'
            else:
                status = 'crashed'
        return LiveRun(data, scrub(err, script, tmp)[:2000], status, code, time.time() - t0)
    finally:
        if proc is not None and proc.poll() is None:
            _kill_group(proc)
            try:
                proc.wait(timeout=5)
            except Exception:
                pass
        if master is not None:
            try:
                os.close(master)
            except OSError:
                pass
        for n in (script, outp):
            try:
                os.unlink(n)
            except OSError:
                pass
        try:
            os.rmdir(tmp)
        except OSError:
            pass
