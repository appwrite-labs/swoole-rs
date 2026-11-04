"""Throwaway #998 diagnostic for x86 CI (never merged): how reset_concurrency's four workers
stop, per engine. Part 1 times a copy with a timestamp beside each WorkerStop echo; part 2
straces the original test and prints every write after DONE."""
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'corpus/upstream'))
import run  # noqa: E402

PHP = ROOT / 'target/php-zts/bin/php'
SOURCE = ROOT / 'target/upstream/src'
CASE = 'tests/swoole_thread/server/reset_concurrency.phpt'
manifest = json.loads((ROOT / 'target/phpt/manifest.json').read_text())
case = next(c for c in manifest['tests'] if c['path'] == CASE)
sandbox = Path(tempfile.mkdtemp(prefix='diag-'))
run.prepare(SOURCE, sandbox, [case], manifest.get('support_files', []))
(sandbox / 'home').mkdir(exist_ok=True)
(sandbox / 'tmp').mkdir(exist_ok=True)
log = sandbox / 'spread.log'
stamp = "file_put_contents('%s', '%%s ' . hrtime(true) . ' ' . %%s . \"\\n\", FILE_APPEND);" % log
text = (sandbox / CASE).read_text()
text = text.replace("    echo 'WORKER STOP', PHP_EOL;",
                    '    ' + stamp % ('stop', '$workerId') + "\n    echo 'WORKER STOP', PHP_EOL;")
text = text.replace('        $serv->shutdown();', '        ' + stamp % ('shutdown', '0') + '\n        $serv->shutdown();')
copy = sandbox / CASE.replace('reset_concurrency', 'rc_spread')
copy.write_text(text)
env = {**os.environ, 'PHPT': '1', 'REPORT_EXIT_STATUS': '1', 'HOME': str(sandbox / 'home'),
       'TMPDIR': str(sandbox / 'tmp'), 'NO_INTERACTION': '1', 'TEST_PHP_EXECUTABLE': str(PHP)}


def runner(engine, test, php=PHP):
    return subprocess.run(
        [str(PHP), str(sandbox / 'run-tests.php'), '-q', '-n', '-p', str(php), '--no-color', '--set-timeout', '20',
         '-d', f'extension={ROOT}/target/phpt/{engine}.so', '-d', 'zend.assertions=1', str(test)],
        capture_output=True, text=True, env=env)


for engine in ('upstream', 'hybrid'):
    firsts, spreads, gaps, fails, orders = [], [], [], 0, []
    for _ in range(30):
        log.write_text('')
        fails += 'FAIL' in runner(engine, copy).stdout
        rows = [line.split() for line in log.read_text().splitlines()]
        if not rows or rows[0][0] != 'shutdown':
            continue
        shutdown = int(rows[0][1])
        stops = sorted((int(r[1]), r[2]) for r in rows[1:5] if r[0] == 'stop')
        if len(stops) != 4:
            continue
        times = [t for t, _ in stops]
        firsts.append((times[0] - shutdown) / 1e3)
        spreads.append((times[-1] - times[0]) / 1e3)
        gaps.append(min(b - a for a, b in zip(times, times[1:])) / 1e3)
        orders.append(''.join(w for _, w in stops))
    med = lambda xs: statistics.median(xs) if xs else float('nan')  # noqa: E731
    print(f'{engine}: copy failed {fails}/30; shutdown->first stop {med(firsts):.0f}us; '
          f'first->last {med(spreads):.0f}us; closest pair {med(gaps):.1f}us; orders {orders}', flush=True)

wrapper = sandbox / 'php-strace.sh'
for engine in ('upstream', 'hybrid'):
    for attempt in range(4):
        trace = sandbox / f'trace-{engine}-{attempt}.txt'
        wrapper.write_text(f'''#!/bin/sh
case "$*" in *reset_concurrency.php*) exec strace -f -tt -e trace=write,sendmsg,futex -o {trace} {PHP} "$@" ;; esac
exec {PHP} "$@"
''')
        wrapper.chmod(0o755)
        result = runner(engine, sandbox / CASE, php=wrapper)
        print(f'== strace {engine} #{attempt}: {"FAIL" if "FAIL" in result.stdout else "PASS"}', flush=True)
        if not trace.exists():
            continue
        lines = trace.read_text().splitlines()
        start = next((i for i, l in enumerate(lines) if '"DONE' in l), None)
        if start is None:
            continue
        for line in lines[start:start + 400]:
            if re.search(r' write\(|sendmsg', line) and 'unfinished' not in line:
                print(line[:110])
