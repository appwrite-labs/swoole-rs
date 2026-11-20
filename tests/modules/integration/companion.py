#!/usr/bin/env python3
"""Run a repository-owned companion of a pinned original that stays RACY and unrun (#5358).

Each fixture beside this script keeps its original's code and assertions where the runner's
environment can't serve the original. Each engine of an exact native/Rust pair must print what
the original expects, as PHP's run-tests.php matches it: the trimmed output, against
EXPECTREGEX's /^...$/s or equal to EXPECT.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

# Each case's original, and why it can't run here.
CASES = {
    # Its echo.py is Python 2 (#1974); /bin/sh echoes the line instead.
    'process_exec': ('tests/swoole_process/process_exec.phpt', 1974),
    # nghttp2.org's httpbin answers 502 (#1986), and the PHPT jobs' httpbin speaks only
    # HTTP/1.1; a Swoole HTTP/2 server on loopback serves the same path instead.
    'connect_twice': ('tests/swoole_http2_client_coro/connect_twice.phpt', 1986),
}


def expectation(original):
    """The original's expected output as a predicate on the trimmed output, and its text."""
    for section, regex in [(b'--EXPECTREGEX--\n', True), (b'--EXPECT--\n', False)]:
        if section in original:
            wanted = original.split(section, 1)[1].decode().strip().replace('\r\n', '\n')
            if regex:
                pattern = re.compile(wanted, re.S)
                return (lambda output: pattern.fullmatch(output) is not None), wanted
            return (lambda output: output == wanted), wanted
    raise ValueError('The original has neither EXPECTREGEX nor EXPECT; review the companion')


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--checkout', type=Path, required=True, help='Checkout providing tools/harness.py')
    p.add_argument('--source', type=Path, required=True, help='Pinned upstream tree with the original')
    p.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension; exact native/Rust pair')
    p.add_argument('--php', default='php')
    p.add_argument('--php-extension', action='append', default=[])
    p.add_argument('--case', choices=sorted(CASES), required=True)
    p.add_argument('--fixture', type=Path, help='Default: CASE.php beside this script')
    p.add_argument('--repetitions', type=int, default=5)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    if a.repetitions < 1:
        p.error('--repetitions must be positive')
    fixture = a.fixture or Path(__file__).with_name(a.case + '.php')
    original_path, issue = CASES[a.case]
    sys.path.insert(0, str(a.checkout.resolve()))
    from tools.harness import parse_engines, run_isolated, write_report
    report = dict(passed=False, executed=False, results=[],
                  scope=f'Repository-owned {a.case} companion only; the original stays RACY and unrun.')
    try:
        engines = parse_engines(a.engine, a.php, a.php_extension)
        metadata = [engine.metadata() for engine in engines]
        report['engines'] = metadata
        if len(engines) != 2 or len({row['name'] for row in metadata}) != 2:
            raise ValueError('Two distinct engine names required')
        if sorted(bool(row['rust_feature_bits']) for row in metadata) != [False, True]:
            raise ValueError('Exact native/Rust engine pair required')
        original = (a.source / original_path).read_bytes()
        matches, expected = expectation(original)
        report['original'] = dict(path=original_path, sha256=hashlib.sha256(original).hexdigest(),
                                  status='racy', issue=issue)
        report['fixture'] = dict(path=str(fixture.resolve()), sha256=sha(fixture), expected=expected)
        for attempt in range(1, a.repetitions + 1):
            for engine in engines:
                report['executed'] = True
                # The originals' Assert:: calls are assert()s here.
                command = engine.command('-d', 'zend.assertions=1', '-d', 'assert.exception=1', fixture)
                row = dict(engine=engine.name, attempt=attempt, command=list(map(str, command)))
                try:
                    observed = run_isolated(command, timeout=15)
                    stdout = observed.stdout.strip().replace('\r\n', '\n')
                    row.update(exit=observed.returncode, stdout=observed.stdout, stderr=observed.stderr,
                               passed=observed.returncode == 0 and not observed.stderr
                               and matches(stdout))
                except subprocess.TimeoutExpired as error:
                    def decoded(value):
                        return value.decode(errors='replace') if isinstance(value, bytes) else value or ''
                    row.update(exit=None, timed_out=True, stdout=decoded(error.stdout),
                               stderr=decoded(error.stderr), passed=False)
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
