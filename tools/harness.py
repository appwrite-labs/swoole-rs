"""Shared process isolation for regression tests, project corpus, and benchmarks."""
import contextlib
from dataclasses import dataclass
import fcntl
import hashlib
import http.client
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]

# Tools and tests address repositories explicitly (git -C). Variables a git hook
# exports would otherwise redirect every child git command to the caller's repository.
for _name in subprocess.run(['git', 'rev-parse', '--local-env-vars'], capture_output=True, text=True).stdout.split():
    os.environ.pop(_name, None)


@dataclass(frozen=True)
class Engine:
    name: str
    extension: Path
    php: str = 'php'
    php_extensions: tuple = ()

    def command(self, *args):
        extra = [arg for extension in self.php_extensions for arg in ['-d', f'extension={extension}']]
        return [self.php, '-n', *extra, '-d', f'extension={self.extension}', *map(str, args)]

    def pid(self, proc):
        """The PHP process PROC, started from command(), runs: PROC itself unless PHP is wrapped."""
        return proc.pid

    def metadata(self):
        if not self.extension.is_file():
            raise ValueError(f'Extension does not exist: {self.extension}')
        code = r'''
        if (!extension_loaded('swoole')) { fwrite(STDERR, 'swoole not loaded'); exit(1); }
        echo json_encode(['php' => PHP_VERSION, 'swoole' => phpversion('swoole'),
          'rust_bridge_abi' => defined('SWOOLE_RS_ABI_VERSION') ? SWOOLE_RS_ABI_VERSION : null,
          'rust_feature_bits' => defined('SWOOLE_RS_FEATURE_BITS') ? SWOOLE_RS_FEATURE_BITS : 0,
          'zts' => PHP_ZTS, 'capabilities' => array_values(array_filter([
          class_exists('Swoole\\Http\\Server') ? 'http' : null,
          function_exists('Swoole\\Coroutine\\run') && class_exists('Swoole\\Coroutine\\Channel') ? 'coroutines' : null,
          class_exists('Swoole\\WebSocket\\Server') ? 'websocket' : null,
          class_exists('Swoole\\Coroutine\\Http\\Client') ? 'coroutine_http_client' : null,
          class_exists('Swoole\\Coroutine\\Client') ? 'coroutine_tcp_client' : null,
          class_exists('Swoole\\Client') ? 'sync_tcp_client' : null,
          extension_loaded('pdo_sqlite') ? 'sqlite' : null,
          function_exists('socket_create') ? 'php_sockets' : null,
          class_exists('Swoole\\Server') ? 'tcp' : null,
          class_exists('Swoole\\Server') ? 'length_framing' : null,
          class_exists('Swoole\\Timer') ? 'timers' : null,
          method_exists('Swoole\\Timer', 'info') ? 'timer_info' : null,
          method_exists('Swoole\\Timer', 'stats') ? 'timer_stats' : null,
          method_exists('Swoole\\Timer', 'list') ? 'timer_list' : null,
          method_exists('Swoole\\Timer', 'clearAll') ? 'timer_clear_all' : null,
          defined('SWOOLE_HAVE_ZLIB') && SWOOLE_HAVE_ZLIB ? 'zlib' : null,
          class_exists('Swoole\\Event') ? 'event_loop' : null,
          class_exists('Swoole\\Table') ? 'table' : null,
          class_exists('Swoole\\Coroutine\\Channel') ? 'channels' : null,
          class_exists('Swoole\\Process') ? 'processes' : null,
          method_exists('Swoole\\Http\\Server', 'taskwait') ? 'task_workers' : null,
          method_exists('Swoole\\Http\\Request', 'create') && method_exists('Swoole\\Http\\Request', 'parse') ? 'http_request_parser' : null,
          method_exists('Swoole\\WebSocket\\Frame', 'pack') ? 'websocket_codec' : null,
          class_exists('Swoole\\Atomic') ? 'atomics' : null,
          PHP_ZTS && defined('SWOOLE_THREAD') && class_exists('Swoole\\Thread') ? 'threads' : null,
          (function () { ob_start(); (new ReflectionExtension('swoole'))->info();
            return str_contains(ob_get_clean(), 'enabled with thread context'); })() ? 'thread_context' : null,
          getenv('ASAN_OPTIONS') !== false ? 'sanitizers' : null,
          function_exists('swoole_typed_array') && function_exists('swoole_call_array_method') ? 'stdext' : null,
          function_exists('swoole_async_set') && method_exists('Swoole\\Coroutine', 'set') && defined('SWOOLE_USE_SHORTNAME') ? 'upstream_test_bootstrap' : null]))]);
        '''
        result = subprocess.run(self.command('-r', code), capture_output=True, text=True, timeout=10)
        if result.returncode:
            raise RuntimeError(result.stdout + result.stderr)
        metadata = json.loads(result.stdout)
        return {**metadata, 'name': self.name, 'extension': str(self.extension),
                'sha256': hashlib.sha256(self.extension.read_bytes()).hexdigest(),
                'php_extensions': list(self.php_extensions)}


def parse_engines(values, php='php', php_extensions=()):
    engines = []
    for value in values:
        name, sep, path = value.partition('=')
        if not sep or not name or not path:
            raise ValueError('Use --engine NAME=/absolute/path/to/extension')
        if any(engine.name == name for engine in engines):
            raise ValueError(f'Duplicate engine: {name}')
        engines.append(Engine(name, Path(path).resolve(), php, tuple(php_extensions)))
    return engines


@contextlib.contextmanager
def server(engine, script, env=None):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryDirectory(prefix='swoole-corpus-') as data:
        proc = subprocess.Popen(engine.command(script),
                                env={**os.environ, 'TEST_PORT': str(port), 'TEST_DATA': data, **(env or {})},
                                stdout=logs, stderr=logs, start_new_session=True)
        try:
            deadline = time.monotonic() + 10
            while True:
                if proc.poll() is not None:
                    raise RuntimeError(f'{engine.name}: PHP exited during startup ({proc.returncode})')
                try:
                    with socket.create_connection(('127.0.0.1', port), timeout=.1):
                        break
                except OSError:
                    if time.monotonic() > deadline:
                        raise RuntimeError(f'{engine.name}: PHP did not listen within 10s')
                    time.sleep(.02)
            yield port, proc
        except BaseException:
            logs.seek(0)
            print(logs.read().decode(errors='replace'), file=sys.stderr)
            raise
        finally:
            # Upstream can fork workers. Kill the entire isolated process group,
            # including on failed startup, assertion failures, and Ctrl+C.
            stop_process_group(proc)


def stop_process_group(proc):
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        proc.wait(timeout=3)
    except subprocess.TimeoutExpired:
        pass
    try:
        os.killpg(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    proc.wait()


@contextlib.contextmanager
def exclusive(path):
    """Fail visibly rather than racing another build or silently waiting on it."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('a') as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError(f'Another writer holds {path}; retry after it finishes') from None
        try:
            yield
        finally:
            fcntl.flock(handle, fcntl.LOCK_UN)


C_COMPILERS = {'cc', 'c++', 'gcc', 'g++', 'clang', 'clang++'}


def run_compiler(command, *, timeout=60):
    """Run a compile command, through ccache for C/C++ compilers when it is installed."""
    started = time.monotonic()
    command = list(map(str, command))
    if Path(command[0]).name in C_COMPILERS and shutil.which('ccache'):
        command = ['ccache', *command]
    result = run_isolated(command, timeout=timeout)
    return {'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr,
            'seconds': round(time.monotonic() - started, 4)}


def run_isolated(command, *, timeout=15, cwd=None, env=None):
    """Capture a CLI test, bounding its whole process group including forked children."""
    proc = subprocess.Popen(command, cwd=cwd, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, errors='replace', start_new_session=True)
    result = None
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
        result = subprocess.CompletedProcess(command, proc.returncode, stdout, stderr)
        return result
    finally:
        try:
            stop_process_group(proc)
        except PermissionError as error:
            if proc.poll() is None:
                raise
            # macOS answers killpg with EPERM when the group's only members are zombies,
            # such as a leader that exits between the SIGTERM wait and the SIGKILL. A run
            # that timed out keeps its TimeoutExpired, which the caller reports.
            if result is not None:
                # Preserve an exited diagnostic's actual failure/output; cleanup
                # uncertainty must also make an otherwise successful run fail.
                result.stderr += f'\nProcess-group cleanup failed: {error}\n'
                result.returncode = result.returncode or 125


def request(port, path='/', method='GET', body=None, headers=None):
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
    try:
        conn.request(method, path, body, headers or {})
        response = conn.getresponse()
        return response.status, dict(response.getheaders()), response.read()
    finally:
        conn.close()


def write_report(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + '\n')
