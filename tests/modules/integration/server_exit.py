#!/usr/bin/env python3
"""Qualify a repository-owned thread-server exit/restart companion on real ZTS engines.

The pinned original remains registered and EXCLUDED under #1120, never passed.
This companion keeps its lifecycle, exit235 and exact stdout assertions, publishes
startup lines before readiness, waits for the replacement worker explicitly, and uses an external controller to request master shutdown after the original 200ms delay.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import uuid


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkout', type=Path, required=True, help='Checkout providing the existing harness/official-runner preparation')
    p.add_argument('--source', type=Path, required=True, help='Read-only pinned Git checkout')
    p.add_argument('--runner', type=Path, required=True, help="Matching PHP's official run-tests.php from phpize")
    p.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension; exact native/Rust pair')
    p.add_argument('--php', default='php')
    p.add_argument('--php-extension', action='append', default=[])
    p.add_argument('--fixture', type=Path, default=Path(__file__).with_suffix('.php'))
    p.add_argument('--repetitions', type=int, default=5)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if a.repetitions < 1:
        p.error('--repetitions must be positive')
    sys.path.insert(0, str(a.checkout.resolve()))
    from corpus.upstream.run import prepare
    from tools.build_upstream import COMMIT
    from tools.harness import parse_engines, run_isolated, write_report
    output = a.output.resolve()
    work = output.parent / ('server-exit-' + uuid.uuid4().hex)
    report = dict(passed=False, executed=False, work=str(work), results=[],
                  scope='Repository-owned real ZTS thread-server exit/restart companion only; original remains excluded, not passed; no other lifecycle/platform/performance qualification.')
    try:
        work.mkdir(parents=True)
        engines = parse_engines(a.engine, a.php, a.php_extension)
        metadata = [engine.metadata() for engine in engines]
        report['engines'] = metadata
        if len(engines) != 2 or len({row['name'] for row in metadata}) != 2:
            raise ValueError('Two distinct engine names required')
        if len({row['php'] for row in metadata}) != 1 or any(row['zts'] != 1 for row in metadata):
            raise ValueError('Same-version real ZTS PHP pair required')
        if sorted(bool(row['rust_feature_bits']) for row in metadata) != [False, True]:
            raise ValueError('Exact native/Rust engine pair required')
        for engine in engines:
            probe = run_isolated(engine.command('-r',
                'exit(PHP_ZTS && defined("SWOOLE_THREAD") && class_exists("Swoole\\\\Thread") ? 0 : 1);'), timeout=10)
            if probe.returncode or probe.stdout or probe.stderr:
                raise ValueError('Real thread-mode capability probe failed: ' + engine.name)
        original_path = 'tests/swoole_thread/server/exit.phpt'
        original = subprocess.check_output(['git', '-C', str(a.source), 'cat-file', 'blob', f'{COMMIT}:{original_path}'])
        expected = original.split(b'--EXPECTF--\n', 1)[1].decode()
        if '%' in expected:
            raise ValueError('Pinned expectation is no longer literal; review companion')
        report['original'] = dict(path=original_path, sha256=hashlib.sha256(original).hexdigest(),
                                  status='excluded', issue=1120)
        report['fixture'] = dict(path=str(a.fixture.resolve()), sha256=sha(a.fixture), expected_stdout=expected)
        source = work / 'source'
        subprocess.run(['git', 'clone', '-q', '--shared', '--no-checkout', str(a.source.resolve()), str(source)], check=True)
        subprocess.run(['git', '-C', str(source), 'checkout', '-q', '--detach', COMMIT], check=True)
        shutil.copy2(a.runner, source / 'run-tests.php')
        sandbox = work / 'sandbox'
        case = dict(path=original_path, sha256=hashlib.sha256(original).hexdigest(), requires=[])
        prepare(source, sandbox, [case])
        fixture = sandbox / 'tests/swoole_thread/server/server_exit_companion.php'
        shutil.copy2(a.fixture, fixture)
        for attempt in range(1, a.repetitions + 1):
            for engine in engines:
                report['executed'] = True
                command = engine.command('-d', 'zend.assertions=1', fixture)
                try:
                    observed = run_isolated(command, timeout=30, cwd=sandbox)
                    row = dict(engine=engine.name, attempt=attempt, command=list(map(str, command)),
                               exit=observed.returncode, stdout=observed.stdout, stderr=observed.stderr,
                               passed=observed.returncode == 0 and observed.stdout == expected and not observed.stderr)
                except subprocess.TimeoutExpired as error:
                    def decoded(value):
                        return value.decode(errors='replace') if isinstance(value, bytes) else value or ''
                    row = dict(engine=engine.name, attempt=attempt, command=list(map(str, command)),
                               exit=None, timed_out=True, stdout=decoded(error.stdout),
                               stderr=decoded(error.stderr), passed=False)
                report['results'].append(row)
                write_report(output, report)
                if not row['passed']:
                    raise ValueError('Companion failed; first output retained, without retry: ' + engine.name)
        for engine, identity in zip(engines, metadata):
            if sha(engine.extension) != identity['sha256']:
                raise ValueError('Engine extension changed during the companion')
        if sha(a.fixture) != report['fixture']['sha256']:
            raise ValueError('Companion fixture changed')
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'executed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
