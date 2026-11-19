#!/usr/bin/env python3
"""swoole/library's own PHPUnit suite against each engine's embedded library.

--prepare checks swoole/library out at the commit the pinned extension embeds (the
`$Id` of ext-src/php_swoole_library.h, so it follows pin moves) and installs its
test tooling from the committed lock. A run then executes every test file per
engine, as the library's own build-swoole workflow does against an extension it
built: swoole.enable_library on, the backing services of its docker-compose.yml on
localhost, counit for the concurrent suite and PHPUnit for the serial one, but one
process per file.

--services starts those services and waits until the slow ones answer. Oracle's
known first-start process-scan failure gets one fresh attempt, with logs preserved.
When one stops
or they don't answer in time, it keeps every container's state and log under
<output>/services/ and exits 1.

The first engine is the control. A test that passes on it and doesn't on another
engine is a regression and exits 1; tests failing on every engine are listed.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import urllib.request
import xml.etree.ElementTree as ET

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT))
from tools.annotations import annotate

HEADER = ROOT / 'target/upstream/src/ext-src/php_swoole_library.h'
SOURCE = ROOT / 'target/library/src'
REPOSITORY = 'https://github.com/swoole/library.git'
# Our copy of composer-embedded.json with PHP pinned to the runners' release, so the
# lock resolves the same everywhere.
COMPOSER = 'composer-swoole-rs.json'
SERVICES = ('consul', 'httpbin', 'mongodb', 'mysql', 'nacos', 'oracle', 'pgsql', 'php-fpm', 'redis')
COMPOSE = ('docker', 'compose', '-f', 'docker-compose.yml', '-f', 'docker-compose.host.yml')


def embedded_commit():
    match = re.search(r'\$Id: ([0-9a-f]{40})', HEADER.read_text(errors='replace'))
    if not match:
        sys.exit(f'No $Id in {HEADER}; run tools/build_upstream.py first')
    return match.group(1)


def git(*args):
    return subprocess.run(['git', '-C', str(SOURCE), *args], check=True,
                          capture_output=True, text=True).stdout.strip()


def prepare(composer):
    commit = embedded_commit()
    if not (SOURCE / '.git').exists() or git('rev-parse', 'HEAD') != commit:
        shutil.rmtree(SOURCE, ignore_errors=True)
        SOURCE.mkdir(parents=True)
        git('init', '-q')
        git('fetch', '-q', '--depth', '1', REPOSITORY, commit)
        git('checkout', '-q', '--detach', 'FETCH_HEAD')
    # PHPUnit 12 reads method attributes. The pinned library still declares its
    # data providers and test dependencies in docblocks.
    attributes = {'dataProvider': 'DataProvider', 'depends': 'Depends'}

    def add_attributes(match):
        names = re.findall(r'@(dataProvider|depends) (\w+)', match[0])
        return match[0] + ''.join(
            f"    #[\\PHPUnit\\Framework\\Attributes\\{attributes[kind]}('{name}')]\n"
            for kind, name in names)

    for path in (SOURCE / 'tests/unit').rglob('*.php'):
        source = path.read_text()
        updated = re.sub(r'    /\*\*(?:(?!\*/).)*\*/\n(?=    public function test)',
                         add_attributes, source, flags=re.S)
        if updated != source:
            path.write_text(updated)
    shutil.copy2(HERE / 'composer.json', SOURCE / COMPOSER)
    shutil.copy2(HERE / 'composer.lock', SOURCE / COMPOSER.replace('.json', '.lock'))
    # Swoole and phpredis need not be loaded in the PHP that runs Composer.
    subprocess.run([composer, 'install', '--no-interaction', '--no-progress', '--ignore-platform-req=ext-*'],
                   cwd=SOURCE, env={**os.environ, 'COMPOSER': COMPOSER}, check=True)
    # The schema migrator retains the old extension element verbatim.
    config = (SOURCE / 'phpunit.xml.dist').read_text().replace('<extension class=', '<bootstrap class=')
    (SOURCE / 'phpunit.xml').write_text(config)
    subprocess.run(['vendor/bin/phpunit', '--migrate-configuration'], cwd=SOURCE, check=True)


def junit(path):
    """{test id: (status, message)} from a JUnit report; nothing if the run died first."""
    try:
        cases = list(ET.parse(path).iter('testcase'))
    except (OSError, ET.ParseError):
        return {}
    outcomes = {}
    for case in cases:
        name = f"{case.get('class') or case.get('classname')}::{case.get('name')}"
        status, message = 'passed', ''
        for child in case:
            if child.tag in ('failure', 'error'):
                status, message = 'failed', (child.text or child.get('message') or '').strip()
                break
            if child.tag == 'skipped':
                status, message = 'skipped', (child.text or child.get('message') or '').strip()
        outcomes[name] = (status, message[:2000])
    return outcomes


def test_files():
    """[(file, runner)]: the serial suite's files under PHPUnit, the rest under counit,
    as phpunit.xml.dist splits them (tests/unit/Thread needs a thread-safe PHP)."""
    config = ET.parse(SOURCE / 'phpunit.xml.dist').getroot()
    serial = {f.text for f in config.iter('file')}
    excluded = {e.text for e in config.iter('exclude')} - serial
    files = []
    for path in sorted((SOURCE / 'tests/unit').rglob('*Test.php')):
        name = str(path.relative_to(SOURCE))
        if not any(name == e or name.startswith(e + '/') for e in excluded):
            files.append((name, 'vendor/bin/phpunit' if name in serial else 'vendor/bin/counit'))
    return files


def run_engine(name, extension, output, php, timeout):
    """Each test file in its own process, so a crash costs only that file's tests."""
    # PHP_INI_SCAN_DIR's leading ':' keeps the distribution's directory (its extensions
    # load first), and children inherit it: the remote object server is spawned as
    # plain `php`, and has to load the same engine.
    ini_dir = output / name
    shutil.rmtree(ini_dir, ignore_errors=True)
    ini_dir.mkdir(parents=True)
    (ini_dir / 'zz-swoole.ini').write_text(f'extension={Path(extension).resolve()}\nmemory_limit=-1\n')
    env = {**os.environ, 'PHP_INI_SCAN_DIR': f':{ini_dir}', 'GITHUB_ACTIONS': 'yes'}
    outcomes = {}
    for test_file, runner in test_files():
        stem = test_file.removeprefix('tests/unit/').removesuffix('.php').replace('/', '.')
        report = ini_dir / f'{stem}.xml'
        try:
            result = subprocess.run([php, runner, '--log-junit', str(report), test_file],
                                    cwd=SOURCE, env=env, capture_output=True, text=True, timeout=timeout)
            code, text = result.returncode, result.stdout + result.stderr
        except subprocess.TimeoutExpired as error:
            code, text = 'timeout', f'{error.stdout or ""}{error.stderr or ""}'
        (ini_dir / f'{stem}.log').write_text(str(text))
        tests = junit(report)
        outcomes.update(tests)
        # The process itself, so a file that died before its report is a failure too.
        outcomes[f'{test_file} (process)'] = ('passed', '') if tests else ('failed', f'exit {code}: {text[-2000:]}')
    return outcomes


def gate(results):
    control, *others = results
    regressions = {}
    for engine in others:
        regressions[engine] = sorted(
            test for test, (status, _) in results[control]['tests'].items()
            if status == 'passed' and results[engine]['tests'].get(test, ('missing',))[0] != 'passed')
    everywhere = sorted(
        test for test in results[control]['tests']
        if all(results[e]['tests'].get(test, ('missing',))[0] == 'failed' for e in results))
    return regressions, everywhere


def summary(results, regressions, everywhere, commit):
    print(f'### swoole/library `{commit[:8]}` PHPUnit suite\n')
    print('| Engine | Passed | Failed | Skipped |')
    print('| --- | ---: | ---: | ---: |')
    for engine, result in results.items():
        counts = [sum(s == k for s, _ in result['tests'].values()) for k in ('passed', 'failed', 'skipped')]
        print(f'| {engine} | {counts[0]} | {counts[1]} | {counts[2]} |')
    for engine, tests in regressions.items():
        print(f'\n**{engine}: {len(tests)} regression(s)** (pass on {next(iter(results))}):')
        for test in tests:
            status, message = results[engine]['tests'].get(test, ('missing', 'not in the report'))
            print(f'- `{test}` {status}: {message.splitlines()[0] if message else ""}')
    if everywhere:
        print(f'\nFailing on every engine ({len(everywhere)}):')
        for test in everywhere:
            print(f'- `{test}`')


def compose(*args):
    return subprocess.run([*COMPOSE, *args], cwd=SOURCE, capture_output=True, text=True)


def answers(url):
    try:
        with urllib.request.urlopen(url, timeout=5):
            return True
    except OSError:
        return False


def ready():
    """Whether the slow services answer: Oracle's log says so, MySQL pings and Nacos serves."""
    return ('DATABASE IS READY TO USE' in compose('logs', 'oracle').stdout
            and compose('exec', '-T', 'mysql', 'mysqladmin', 'ping', '-uusername', '-ppassword',
                        '--silent').returncode == 0
            and answers('http://127.0.0.1:8848/nacos/'))


def stopped():
    """The services whose container exited or died."""
    return compose('ps', '-a', '--status', 'exited', '--status', 'dead', '--format', '{{.Service}}').stdout.split()


def keep_evidence(directory):
    directory.mkdir(parents=True, exist_ok=True)
    listing = compose('ps', '-a').stdout
    ids = compose('ps', '-a', '-q').stdout.split()
    states = subprocess.run(
        ['docker', 'inspect', '--format', '{{.Name}} status={{.State.Status}} exit={{.State.ExitCode}} '
         'oom={{.State.OOMKilled}} error={{.State.Error}} started={{.State.StartedAt}} '
         'finished={{.State.FinishedAt}}', *ids],
        capture_output=True, text=True).stdout if ids else ''
    (directory / 'ps.txt').write_text(listing)
    (directory / 'state.txt').write_text(states)
    print(listing + states, end='')
    for service in SERVICES:
        logs = compose('logs', '--no-color', '--timestamps', service)
        (directory / f'{service}.log').write_text(logs.stdout + logs.stderr)
    print(f'Service logs: {directory}')


def start_services(evidence, attempts=120, interval=5):
    subprocess.run([*COMPOSE, 'up', '-d', *SERVICES], cwd=SOURCE, check=True)
    recovered = False
    for _ in range(attempts):
        if gone := stopped():
            print(f'Stopped before the services were ready: {", ".join(gone)}')
            # This first-start failure reports a disappeared process (#4624).
            # Recover only that signature, before any tests run, and keep its evidence.
            log = compose('logs', 'oracle').stdout if gone == ['oracle'] else ''
            if not recovered and all(line in log for line in (
                    'ORA-00442:', 'ORA-27300: OS system dependent operation:read failed with status: 3',
                    'ORA-27301: OS failure message: No such process',
                    'ORA-27302: failure occurred at: sxecheck2')):
                keep_evidence(evidence / 'oracle-first-start')
                recovered = True
                message = 'Recreating Oracle once after its first-start process-scan failure'
                print(message)
                annotate('warning', 'Oracle startup retry', message)
                if compose('up', '-d', '--no-deps', '--force-recreate',
                           '--renew-anon-volumes', 'oracle').returncode == 0:
                    continue
            break
        if ready():
            return True
        time.sleep(interval)
    else:
        print(f'The services were not ready after {attempts * interval}s')
    keep_evidence(evidence)
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--prepare', action='store_true')
    parser.add_argument('--services', action='store_true')
    parser.add_argument('--composer', default='composer')
    parser.add_argument('--engine', action='append', default=[], metavar='NAME=PATH',
                        help='first one is the control')
    parser.add_argument('--php', default='php')
    parser.add_argument('--timeout', type=int, default=300, help='seconds per test file')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/library')
    args = parser.parse_args()
    if args.prepare:
        prepare(args.composer)
    if args.services and not start_services(args.output / 'services'):
        sys.exit(1)
    if not args.engine:
        return
    if not (SOURCE / 'vendor/bin/counit').exists():
        sys.exit('swoole/library is not prepared; run corpus/library/run.py --prepare')
    results = {}
    for value in args.engine:
        name, _, path = value.partition('=')
        results[name] = {'extension': path, 'tests': run_engine(name, path, args.output, args.php, args.timeout)}
    regressions, everywhere = gate(results)
    commit = git('rev-parse', 'HEAD')
    (args.output / 'library.json').write_text(json.dumps(
        {'commit': commit, 'engines': results, 'regressions': regressions, 'failing_everywhere': everywhere},
        indent=2) + '\n')
    summary(results, regressions, everywhere, commit)
    control = next(iter(results.values()))['tests'].values()
    if not any(status == 'passed' for status, _ in control):
        sys.exit('The control passed nothing, so the gate means nothing')
    sys.exit(1 if any(regressions.values()) else 0)


if __name__ == '__main__':
    main()
