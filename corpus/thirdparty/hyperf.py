#!/usr/bin/env python3
"""Pinned unchanged Hyperf app plus explicitly external framework probes."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.harness import ROOT, parse_engines, request, run_isolated, stop_process_group, write_report

HERE = Path(__file__).resolve().parent
MANIFEST = json.loads((HERE / 'manifest.json').read_text())
SOURCE = ROOT / 'target/thirdparty/hyperf'
PORT = 9501


def digest(data):
    return hashlib.sha256(data).hexdigest()


def git(*args):
    return subprocess.check_output(['git', '-C', str(SOURCE), *args])


def cloned():
    # A cache restoring target/ can bring the tree back with no or a partial .git; git
    # would then resolve the enclosing checkout, so only a tree that is its own top counts.
    result = subprocess.run(['git', '-C', str(SOURCE), 'rev-parse', '--show-toplevel'],
                            capture_output=True, text=True)
    return result.returncode == 0 and Path(result.stdout.strip()).resolve() == SOURCE.resolve()


def require_equal(actual, expected):
    if actual != expected:
        raise AssertionError(f'expected {expected!r}, got {actual!r}')


def archive():
    if not cloned():
        raise RuntimeError('Pinned Hyperf missing; run corpus/thirdparty/hyperf.py --prepare')
    require_equal(git('rev-parse', 'HEAD').decode().strip(), MANIFEST['commit'])
    require_equal(git('diff', '--binary', 'HEAD'), b'')
    data = git('archive', MANIFEST['commit'])
    require_equal(digest(data), MANIFEST['archive_sha256'])
    require_equal(digest((HERE / 'composer.lock').read_bytes()), MANIFEST['composer_lock_sha256'])
    require_equal(digest((SOURCE / 'composer.lock').read_bytes()), MANIFEST['composer_lock_sha256'])
    installed = json.loads((SOURCE / 'vendor/composer/installed.json').read_text())['packages']
    expected = json.loads((HERE / 'composer.lock').read_text())['packages']
    identity = lambda packages: sorted((p['name'], p['version'], p.get('source', {}).get('reference')) for p in packages)
    require_equal(identity(installed), identity(expected))
    return data


def prepare(composer):
    SOURCE.parent.mkdir(parents=True, exist_ok=True)
    if SOURCE.exists() and not cloned():
        shutil.rmtree(SOURCE)
    if not SOURCE.exists():
        subprocess.run(['git', 'clone', '--no-checkout', MANIFEST['repository'], str(SOURCE)], check=True)
        subprocess.run(['git', '-C', str(SOURCE), 'checkout', '--detach', MANIFEST['commit']], check=True)
    require_equal(git('rev-parse', 'HEAD').decode().strip(), MANIFEST['commit'])
    require_equal(git('diff', '--binary', 'HEAD'), b'')
    shutil.copy2(HERE / 'composer.lock', SOURCE / 'composer.lock')
    subprocess.run([composer, 'install', '--no-dev', '--no-scripts', '--no-plugins',
                    '--prefer-dist', '--no-interaction'], cwd=SOURCE, check=True)
    archive()


def available():
    with socket.socket() as sock:
        # The unchanged app binds all interfaces; refuse to disturb any listener.
        sock.bind(('0.0.0.0', PORT))


def closed():
    try:
        with socket.create_connection(('127.0.0.1', PORT), timeout=.1):
            return False
    except OSError:
        return True


def http_tests():
    cases = [
        ('GET', '/', None, {}, 200, {'method': 'GET', 'message': 'Hello Hyperf.'}),
        ('GET', '/?user=Alice%20%CE%BB', None, {}, 200, {'method': 'GET', 'message': 'Hello Alice λ.'}),
        ('POST', '/', '{"user":"JSON λ"}'.encode(), {'Content-Type': 'application/json'}, 200,
         {'method': 'POST', 'message': 'Hello JSON λ.'}),
        ('POST', '/', b'user=form+body', {'Content-Type': 'application/x-www-form-urlencoded'}, 200,
         {'method': 'POST', 'message': 'Hello form body.'}),
    ]
    for method, path, body, headers, status, expected in cases:
        actual_status, _, payload = request(PORT, path, method, body, headers)
        require_equal(actual_status, status)
        require_equal(json.loads(payload), expected)
    require_equal(request(PORT, '/missing')[0], 404)
    require_equal(request(PORT, '/favicon.ico')[::2], (200, b''))
    return len(cases) + 2


def run(engine):
    original = archive()
    available()
    checks = []
    with tempfile.TemporaryDirectory(prefix='swoole-hyperf-') as tmp:
        app = Path(tmp) / 'app'
        app.mkdir()
        with tarfile.open(fileobj=io.BytesIO(original)) as packed:
            for member in packed.getmembers():
                path = Path(member.name)
                if path.is_absolute() or '..' in path.parts or member.issym() or member.islnk():
                    raise ValueError('unsafe archive entry: ' + member.name)
            packed.extractall(app)
        shutil.copy2(HERE / 'composer.lock', app / 'composer.lock')
        shutil.copytree(SOURCE / 'vendor', app / 'vendor', symlinks=True)
        (app / 'runtime').mkdir(exist_ok=True)
        db = Path(tmp) / 'persistent.sqlite'
        db.touch()
        env = {**os.environ, 'APP_ENV': 'test', 'TEST_APP': str(app),
               'DB_DRIVER': 'sqlite', 'DB_DATABASE': str(db)}
        with tempfile.TemporaryFile() as logs:
            proc = subprocess.Popen(engine.command('-d', 'swoole.use_shortname=Off',
                                     app / 'bin/hyperf.php', 'start'), cwd=app,
                                    env=env, stdout=logs, stderr=logs, start_new_session=True)
            try:
                deadline = time.monotonic() + 30
                while True:
                    if proc.poll() is not None:
                        raise RuntimeError(f'Hyperf startup exited {proc.returncode}')
                    try:
                        if request(PORT, '/')[0] == 200:
                            break
                    except OSError:
                        pass
                    if time.monotonic() > deadline:
                        raise TimeoutError('Hyperf did not become healthy in 30 seconds')
                    time.sleep(.05)
                checks.append({'http_cases': http_tests()})
                for mode in ('write', 'read'):
                    result = run_isolated(engine.command('-d', 'swoole.use_shortname=Off',
                                          HERE / 'probe.php', mode), cwd=app, env=env, timeout=30)
                    if result.returncode:
                        raise RuntimeError(f'{mode} probe exited {result.returncode}: {result.stdout}\n{result.stderr}')
                    actual = json.loads(result.stdout)
                    require_equal(actual, {'mode': mode, 'rows': 1,
                        'value_hex': 'persisted λ\0bytes'.encode().hex(), 'coroutine': True})
                    checks.append(actual)
                require_equal(request(PORT, '/')[::2], (200, b'{"method":"GET","message":"Hello Hyperf."}'))
                # Graceful shutdown is part of the test; fallback group cleanup is unconditional.
                os.kill(engine.pid(proc), signal.SIGTERM)
                require_equal(proc.wait(timeout=15), 0)
                deadline = time.monotonic() + 5
                while not closed() and time.monotonic() < deadline:
                    time.sleep(.05)
                require_equal(closed(), True)
                checks.append({'shutdown_exit': 0, 'port_closed': True})
            except BaseException:
                logs.seek(0)
                print(logs.read().decode(errors='replace'), file=sys.stderr)
                raise
            finally:
                stop_process_group(proc)
        # Assert tracked app files stayed unchanged, excluding runtime/vendor/lock.
        with tarfile.open(fileobj=io.BytesIO(original)) as packed:
            for member in packed.getmembers():
                if member.isfile():
                    require_equal((app / member.name).read_bytes(), packed.extractfile(member).read())
        # Persistence must outlive the entire server, then read in a new process.
        result = run_isolated(engine.command('-d', 'swoole.use_shortname=Off', HERE / 'probe.php', 'read'),
                              cwd=app, env=env, timeout=30)
        require_equal(result.returncode, 0)
        require_equal(json.loads(result.stdout), {'mode': 'read', 'rows': 1,
            'value_hex': 'persisted λ\0bytes'.encode().hex(), 'coroutine': True})
        checks.append({'post_shutdown_persistence': True})
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepare', action='store_true', help='Explicit network/dependency preparation only')
    parser.add_argument('--composer', default='composer')
    parser.add_argument('--engine', action='append')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', default=str(ROOT / 'target/reports/thirdparty-hyperf.json'))
    args = parser.parse_args()
    if args.prepare:
        prepare(args.composer)
        if not args.engine:
            return 0
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'application': MANIFEST,
              'driver_sha256': digest(Path(__file__).read_bytes()),
              'probe_sha256': digest((HERE / 'probe.php').read_bytes()), 'engines': [], 'results': [],
              'scope': MANIFEST['coverage'], 'fixed_port': PORT}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        row = {'engine': engine.name}
        try:
            report['engines'].append(engine.metadata())
            row.update(status='passed', checks=run(engine))
        except Exception as error:
            row.update(status='failed', detail=f'{type(error).__name__}: {error}')
        report['results'].append(row)
        print(json.dumps(row), flush=True)
    write_report(args.output, report)
    return int(any(row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
