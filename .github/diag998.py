"""Throwaway #998 diagnostic for x86 CI (never merged). Runs the unmodified
reset_concurrency.phpt per engine: untraced for a pass rate, then under bpftrace
(syscalls and context switches of `php` threads) to time the four workers' two writes."""
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

ROOT = Path.cwd()
sys.path.insert(0, str(ROOT / 'corpus/upstream'))
import run  # noqa: E402

PHP = ROOT / 'target/php-zts/bin/php'
SOURCE = ROOT / 'target/upstream/src'
CASE = 'tests/swoole_thread/server/reset_concurrency.phpt'
UNTRACED = int(os.environ.get('UNTRACED', '40'))
TRACED = int(os.environ.get('TRACED', '30'))
manifest = json.loads((ROOT / 'target/phpt/manifest.json').read_text())
case = next(c for c in manifest['tests'] if c['path'] == CASE)
sandbox = Path(tempfile.mkdtemp(prefix='diag-'))
run.prepare(SOURCE, sandbox, [case], manifest.get('support_files', []))
(sandbox / 'home').mkdir(exist_ok=True)
(sandbox / 'tmp').mkdir(exist_ok=True)
env = {**os.environ, 'PHPT': '1', 'REPORT_EXIT_STATUS': '1', 'HOME': str(sandbox / 'home'),
       'TMPDIR': str(sandbox / 'tmp'), 'NO_INTERACTION': '1', 'TEST_PHP_EXECUTABLE': str(PHP)}
NAMES = {0: 'read', 1: 'write', 3: 'close', 7: 'poll', 9: 'mmap', 10: 'mprotect', 11: 'munmap', 12: 'brk',
         13: 'sigaction', 14: 'sigprocmask', 20: 'writev', 28: 'madvise', 35: 'nanosleep', 39: 'getpid',
         42: 'connect', 44: 'sendto', 45: 'recvfrom', 46: 'sendmsg', 47: 'recvmsg', 48: 'shutdown', 60: 'exit',
         62: 'kill', 72: 'fcntl', 186: 'gettid', 200: 'tkill', 202: 'futex', 230: 'clock_nanosleep',
         231: 'exit_group', 232: 'epoll_wait', 233: 'epoll_ctl', 234: 'tgkill', 257: 'openat', 262: 'fstatat',
         273: 'set_robust_list', 281: 'epoll_pwait', 283: 'timerfd_create', 286: 'timerfd_settime',
         290: 'eventfd2', 291: 'epoll_create1', 293: 'pipe2', 302: 'prlimit64', 318: 'getrandom',
         334: 'rseq', 435: 'clone3', 441: 'epoll_pwait2', 56: 'clone', 228: 'clock_gettime'}
BT = r'''
tracepoint:raw_syscalls:sys_enter /comm == "php"/ { printf("E %llu %d %d %ld\n", nsecs, pid, tid, args->id); }
tracepoint:raw_syscalls:sys_exit /comm == "php"/ { printf("X %llu %d %d %ld %ld\n", nsecs, pid, tid, args->id, args->ret); }
tracepoint:syscalls:sys_enter_write /comm == "php" && args->fd <= 2/ { printf("W %llu %d %d %r\n", nsecs, pid, tid, buf(args->buf, 12)); }
tracepoint:sched:sched_switch /comm == "php"/ { printf("O %llu %d %d %ld %d\n", nsecs, pid, tid, args->prev_state, args->next_pid); }
tracepoint:sched:sched_switch /str(args->next_comm) == "php"/ { printf("I %llu %d %d\n", nsecs, args->prev_pid, args->next_pid); }
tracepoint:sched:sched_wakeup /str(args->comm) == "php"/ { printf("K %llu %d %d %d\n", nsecs, tid, args->pid, args->target_cpu); }
'''


def runner(engine):
    out = subprocess.run(
        [str(PHP), str(sandbox / 'run-tests.php'), '-q', '-n', '-p', str(PHP), '--no-color', '--set-timeout', '20',
         '-d', f'extension={ROOT}/target/phpt/{engine}.so', '-d', 'zend.assertions=1', str(sandbox / CASE)],
        capture_output=True, text=True, env=env).stdout
    return 'PASS' in out and 'FAIL' not in out


def med(xs):
    return statistics.median(xs) if xs else float('nan')


def analyse(lines, verbose):
    events = []
    for line in lines:
        p = line.split(' ', 4)
        if p[0] in 'EXWO' and len(p) >= 4:
            events.append(p)
    done = next((e for e in events if e[0] == 'W' and e[4].startswith('DONE')), None)
    if not done:
        return None
    pid, t0 = done[2], int(done[1])
    ev = [e for e in events if e[2] == pid and int(e[1]) >= t0 - 3_000_000]
    workers = []
    for e in ev:
        if e[0] == 'W' and e[4].startswith('WORKER STOP') and e[3] not in workers:
            workers.append(e[3])
    stats = []
    for tid in workers:
        mine = [e for e in ev if e[3] == tid]
        i1 = next(i for i, e in enumerate(mine) if e[0] == 'W' and e[4].startswith('WORKER'))
        w1 = int(mine[i1][1])
        x1 = next((int(e[1]) for e in mine[i1:] if e[0] == 'X' and e[4].startswith('1 ')), None)
        w2 = next((int(e[1]) for e in mine[i1 + 1:] if e[0] == 'W'), None)
        prev = [e for e in mine[:i1] if e[0] == 'X']
        wake = int(prev[-1][1]) if prev else None
        prev_id = NAMES.get(int(prev[-1][4].split()[0]), prev[-1][4]) if prev else '?'
        blocked = sum(1 for e in mine[i1:] if e[0] == 'O' and (w2 is None or int(e[1]) < w2))
        stats.append(dict(w1=w1, x1=x1, w2=w2, wake=wake, prev=prev_id, blocked=blocked))
    if verbose:
        print(f'  timeline from 1 ms before the first WORKER STOP (us); workers {workers}')
        first = min(s['w1'] for s in stats) if stats else t0
        for e in ev:
            t = int(e[1])
            if e[3] in workers and first - 1_000_000 <= t <= first + 1_000_000:
                what = e[4] if e[0] == 'W' else (NAMES.get(int(e[4].split()[0]), e[4]) + (' ret ' + e[4].split()[1] if e[0] == 'X' else '')) if e[0] in 'EX' else e[4]
                print(f'    {(t - first) / 1e3:9.2f} {e[3]} {e[0]} {what}')
    return stats


summary = []
for engine in ('upstream', 'hybrid'):
    passes = sum(runner(engine) for _ in range(UNTRACED))
    summary.append(f'{engine}: untraced passed {passes}/{UNTRACED}')
    print(summary[-1], flush=True)

subprocess.run(['sudo', 'sysctl', '-q', 'kernel.perf_event_paranoid=-1'])
for engine in ('upstream', 'hybrid'):
    passes, gaps, durs, spreads, wakes, prevs, blocked = 0, [], [], [], [], defaultdict(int), 0
    for attempt in range(TRACED):
        log = sandbox / f'bt-{engine}-{attempt}.txt'
        bt = subprocess.Popen(['sudo', 'env', 'BPFTRACE_PERF_RB_PAGES=4096', 'bpftrace', '-q', '-e', BT],
                              stdout=open(log, 'w'), stderr=subprocess.STDOUT)
        time.sleep(4)
        ok = runner(engine)
        time.sleep(0.5)
        subprocess.run(['sudo', 'kill', '-INT', str(bt.pid)])
        bt.wait()
        passes += ok
        stats = analyse(log.read_text(errors='replace').splitlines(), attempt < 3)
        if not stats or len(stats) != 4:
            print(f'  {engine} #{attempt} {"PASS" if ok else "FAIL"}: {len(stats or [])} workers seen; '
                  + log.read_text(errors='replace')[:300].replace('\n', ' | '))
            continue
        print(f'  {engine} #{attempt} {"PASS" if ok else "FAIL"}: ' + '; '.join(
            f'w1 +{(s["w1"] - min(x["w1"] for x in stats)) / 1e3:.1f} dur {((s["x1"] or 0) - s["w1"]) / 1e3:.1f} '
            f'gap {((s["w2"] or 0) - (s["x1"] or 0)) / 1e3:.1f} wake->w1 {((s["w1"] - (s["wake"] or s["w1"]))) / 1e3:.1f} after {s["prev"]} sw {s["blocked"]}'
            for s in sorted(stats, key=lambda s: s['w1'])), flush=True)
        for s in stats:
            if s['x1'] and s['w2']:
                gaps.append((s['w2'] - s['x1']) / 1e3)
                durs.append((s['x1'] - s['w1']) / 1e3)
            if s['wake']:
                wakes.append((s['w1'] - s['wake']) / 1e3)
            prevs[s['prev']] += 1
            blocked += s['blocked']
        spreads.append((max(s['w1'] for s in stats) - min(s['w1'] for s in stats)) / 1e3)
        log.unlink()
    summary.append(f'{engine}: traced passed {passes}/{TRACED}; write1 dur {med(durs):.1f}us; write1 exit->write2 '
                   f'{med(gaps):.1f}us; first->last write1 {med(spreads):.1f}us; prev syscall exit->write1 '
                   f'{med(wakes):.1f}us; prev syscalls {dict(prevs)}; switches between writes {blocked}')
print('\n'.join(summary))
