"""Run Luau code in a resource-limited child process (used only for the patched loader that dumps the VM)."""
import os
import subprocess
import tempfile

from .astutil import find_bin
from .messages import scrub


class SandboxError(Exception):
    def __init__(self, code, message):
        Exception.__init__(self, message)
        self.code = code
        self.message = message


def _limits(mem_bytes, cpu_seconds, fsize_bytes):
    def apply():
        try:
            import resource
            if mem_bytes:
                resource.setrlimit(resource.RLIMIT_AS, (mem_bytes, mem_bytes))
            if cpu_seconds:
                resource.setrlimit(resource.RLIMIT_CPU, (cpu_seconds, cpu_seconds + 1))
            if fsize_bytes:
                resource.setrlimit(resource.RLIMIT_FSIZE, (fsize_bytes, fsize_bytes))
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
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
