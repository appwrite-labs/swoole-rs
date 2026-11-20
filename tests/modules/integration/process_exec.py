#!/usr/bin/env python3
"""Run the repository-owned companion of the pinned tests/swoole_process/process_exec.phpt.

The original stays RACY and unrun (its echo.py is Python 2, #1974). This companion keeps its
Swoole\\Process exec, pipe and expected output, with /bin/sh as the child (#5358). Each engine
of an exact native/Rust pair must print what the original's EXPECTREGEX accepts, as PHP's
run-tests.php matches it: the trimmed output, against /^...$/s.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

ORIGINAL = 'tests/swoole_process/process_exec.phpt'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkout', type=Path, required=True, help='Checkout providing tools/harness.py')
    p.add_argument('--source', type=Path, required=True, help='Pinned upstream tree with the original')
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
    from tools.harness import parse_engines, run_isolated, write_report
    report = dict(passed=False, executed=False, results=[],
                  scope='Repository-owned process_exec companion only; the original stays RACY and unrun.')
    try:
        engines = parse_engines(a.engine, a.php, a.php_extension)
        metadata = [engine.metadata() for engine in engines]
        report['engines'] = metadata
        if len(engines) != 2 or len({row['name'] for row in metadata}) != 2:
            raise ValueError('Two distinct engine names required')
        if sorted(bool(row['rust_feature_bits']) for row in metadata) != [False, True]:
            raise ValueError('Exact native/Rust engine pair required')
        original = (a.source / ORIGINAL).read_bytes()
        expected = original.split(b'--EXPECTREGEX--\n', 1)[1].decode().strip().replace('\r\n', '\n')
        report['original'] = dict(path=ORIGINAL, sha256=hashlib.sha256(original).hexdigest(), status='racy', issue=1974)
        report['fixture'] = dict(path=str(a.fixture.resolve()), sha256=sha(a.fixture), expected_regex=expected)
        pattern = re.compile(expected, re.S)
        for attempt in range(1, a.repetitions + 1):
            for engine in engines:
                report['executed'] = True
                command = engine.command(a.fixture)
                observed = run_isolated(command, timeout=15)
                stdout = observed.stdout.strip().replace('\r\n', '\n')
                row = dict(engine=engine.name, attempt=attempt, command=list(map(str, command)),
                           exit=observed.returncode, stdout=observed.stdout, stderr=observed.stderr,
                           passed=observed.returncode == 0 and not observed.stderr
                           and pattern.fullmatch(stdout) is not None)
                report['results'].append(row)
                write_report(a.output, report)
                if not row['passed']:
                    raise ValueError('Companion failed; first output retained, without retry: ' + engine.name)
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(a.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'executed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
