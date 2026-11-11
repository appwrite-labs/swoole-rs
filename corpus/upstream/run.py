#!/usr/bin/env python3
"""Run pinned original Swoole PHPT cases with PHP's official test runner."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shutil
import shlex
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.build_upstream import COMMIT, VERSION
from tools.harness import ROOT, parse_engines, run_isolated, write_report
from tools.phpt_expectations import select

# Only local upstream test helpers are autoloaded. Curated cases must not need
# Composer packages or external services. Test files and assertions are untouched.
AUTOLOAD = r'''<?php
spl_autoload_register(static function (string $class): void {
    $prefix = 'SwooleTest\\';
    if (str_starts_with($class, $prefix)) {
        $file = __DIR__ . '/../src/' . str_replace('\\', '/', substr($class, strlen($prefix))) . '.php';
        if (is_file($file)) { require $file; }
    }
});
'''


# Audited pinned cases use only process-local state, with no listeners or shared files.
# New cases stay isolated until explicitly reviewed here.
PARALLEL_SAFE = frozenset(['tests/swoole_channel_coro/basic.phpt', 'tests/swoole_channel_coro/close.phpt', 'tests/swoole_channel_coro/pop_after_close.phpt', 'tests/swoole_channel_coro/chan_stats.phpt', 'tests/swoole_channel_coro/type.phpt', 'tests/swoole_channel_coro/push_close1.phpt', 'tests/swoole_table/int.phpt', 'tests/swoole_table/negative.phpt', 'tests/swoole_table/del.phpt', 'tests/swoole_table/set_after_del.phpt', 'tests/swoole_table/incr_after_del.phpt', 'tests/swoole_table/type_conv.phpt', 'tests/swoole_table/key_value.phpt', 'tests/swoole_table/foreach.phpt', 'tests/swoole_coroutine/empty.phpt', 'tests/swoole_coroutine/current.phpt', 'tests/swoole_coroutine/getPcid.phpt', 'tests/swoole_coroutine/defer/defer.phpt', 'tests/swoole_coroutine/callback.phpt', 'tests/swoole_coroutine/array_walk.phpt', 'tests/swoole_coroutine/eval.phpt', 'tests/swoole_atomic/atomic.phpt', 'tests/swoole_http_server/create_request.phpt', 'tests/swoole_websocket_server/pack.phpt'])


@contextmanager
def timed_phase(report, phase):
    """Add wall time without changing correctness outcomes or execution policy."""
    started = time.monotonic()
    try:
        yield
    finally:
        timings = report.setdefault('timings_seconds', {})
        timings[phase] = round(timings.get(phase, 0) + time.monotonic() - started, 4)


# Cases that bind a fixed address. A live listener there fails the case before it
# runs, so it can never talk to an unrelated process; TIME_WAIT is tolerated.
FIXED_PORTS = {'tests/swoole_timer/bug_4794_8.phpt': 9601}

# Cases whose SKIPIF's only bar is skip_if_in_ci() and that need nothing the PHPT jobs lack
# (the ssh2 key login phpt-env sets up). They run with the CI marker removed.
OUTSIDE_CI = {'tests/swoole_ssh2/ssh2_auth_pubkey.phpt', 'tests/swoole_ssh2/ssh2_auth_pubkey_file.phpt'}


# Only this named ZTS original is excluded after the same output interleaving was
# reproduced on native and hybrid (#1120). Its registered source/hash is retained;
# the repository-owned server_exit companion qualifies lifecycle behavior instead.
EXCLUDED = {'tests/swoole_thread/server/exit.phpt':
            'two WorkerStart echo operands interleave on native and hybrid; native attempt44 '
            'and hybrid attempts4/17/30/32 in the paired45 stress reproduce worker '
            'startworker start (#1120); lifecycle companion: tests/modules/integration/server_exit.py'}


def named_exclusion(case, metadata):
    """A named original is excluded only on the verified runtime where its output races."""
    if metadata.get('zts') == 1 and 'threads' in metadata.get('capabilities', []):
        return EXCLUDED.get(case['path'])
    return None


def verdict_failed(row, require_all):
    """Named exclusions stay non-passing; a real failure is never waived."""
    if row['status'] == 'failed':
        return True
    return (require_all and row['status'] != 'passed' and row['test'] not in RACY | THREAD_CONTEXT_RACY | SANITIZER_RACY
            and not (row['status'] == 'excluded' and row['test'] in EXCLUDED))


# Originals whose verdict is decided by something outside the engine, on upstream too: a
# race in the test itself, the runner's network, or a file or tool the runner can't use. They
# are recorded unsupported, never run or counted as passed.
RACY = {'tests/swoole_http_server/compression_content_type_release.phpt':
        'the file starts with a blank line, so PHP\'s runner BORKs it ("tests must start with '
        '--TEST--") on every engine (#1964)',
        'tests/swoole_process/process_exec.phpt':
        "it execs /usr/bin/python on echo.py, which is Python 2 (raw_input, the print "
        'statement); Python 3 rejects it with a SyntaxError on every engine (#1974)',
        'tests/swoole_runtime/exec/exec.phpt':
        "it pings www.gov.cn three times through each exec function; GitHub's hosted runners "
        'drop ICMP, so ping never answers and the case times out on every engine (#1974)',
        'tests/swoole_http2_client_coro/connect_twice.phpt':
        "it fetches https://nghttp2.org/httpbin/base64/..., and nghttp2.org's httpbin backend "
        'answers 502 Bad Gateway (nghttpx) to every engine and to curl (#1986)',
        'tests/swoole_curl/close_before_resume.phpt':
        "the expected Swoole\\Error is fatal_error(), which exits 255 without PHP's shutdown "
        "functions, so ProcessManager never kills its SWOOLE_PROCESS server; the orphaned server "
        "holds the runner's stdout open and the runner, upstream's run-tests too, waits out its "
        'deadline on every engine (#1994)',
        'tests/swoole_curl/exec_twice.phpt':
        'the same fatal_error() exit as close_before_resume orphans its SWOOLE_PROCESS server, '
        "which holds the runner's stdout open until the deadline on every engine (#1994)",
        'tests/swoole_ssh2/ssh2_sftp_001.phpt':
        "libssh2 (1.11.0 and 1.11.1) against OpenSSH's sftp-server (9.6, 10.0) fails every "
        "SSH_FXP_SYMLINK after an SFTP channel's first request, though the link is created; "
        'plain PECL ssh2 and a C client do the same, and the test calls mkdir first (#1994)',
        'tests/swoole_table/force_unlock.phpt':
        "it expects the child's OOM to bail out of Table::get while it holds the row lock; PHP's "
        'fix for CVE-2026-109618 (php-src 3584a98, in the runners\' PHP 8.3.6) delays a memory '
        'limit error until the internal call returns, so get() unlocks the row first (#1994)',
        'tests/swoole_websocket_server/send_close_frame.phpt':
        'the parent kills the server as soon as its pushes return, usually before the server '
        'handles the second frame (#111)',
        'tests/swoole_coroutine/dnslookup_ipv6.phpt':
        "the verdict depends on the runner's resolver reply, and both engines fail on some "
        'replies (#428)',
        'tests/swoole_process_pool/getprocess_4.phpt':
        'worker 0 gives the pool a fixed 100 ms to restart the SIGTERMed worker 1 before '
        'getProcess(1); on a loaded runner both engines miss it (upstream failed re-runs in 3 '
        'merge-tier runs, #703)',
        'tests/swoole_curl/multi/bug4393.phpt':
        'it asserts two concurrent requests to live www.qq.com and www.baidu.com finish within '
        "3.0 s; that's the runner's WAN latency, and upstream misses it too (9.23 s, 3.26 s, "
        '3.17 s in 3 of 20 merge-tier runs; hybrid 6.66 s in 1, #1210)',
        'tests/swoole_runtime/nonblock.phpt':
        'it connects to live www.baidu.com:80 with a 1 s timeout, and every engine sometimes '
        'misses it with the same "Unable to connect" (16 merge-tier runs: upstream 3/32, hybrid '
        '4/32, cargo 2/15 first runs; hybrid 2/4 then upstream 0/25 in one iouring gate, #2116)',
        'tests/swoole_client_async/enableSSL.phpt':
        "it asserts live www.baidu.com's reply contains 'www.baidu.com' in the first onReceive, "
        'which gets whatever TLS records have arrived: the server splits the reply into records '
        '(789, 1024, 907, 395 bytes...) sometimes 0.1-25 ms apart, and a first read of only the '
        "789-byte record fails on every engine (aarch64 dev image, alternating: upstream 41/60, "
        'cargo 48/60 idle and under CPU load; merge tier cargo 2/4 then upstream 0/27, #2873)',
        'tests/swoole_websocket_server/pack.phpt':
        'mt_rand(0, 128000) can draw 0, and get_safe_random(0) reaches '
        'openssl_random_pseudo_bytes(0), which throws ValueError: about 0.8% of runs on any '
        'engine (#321)',
        'tests/swoole_websocket_server/compression_incompressible.phpt':
        "disconnect() pushes the close frame, then closes the session; the client closes on "
        "receiving it, and when the reactor thread handles that first the session is gone and "
        'disconnect() returns false (#321)',
        'tests/swoole_server/close_max_fd.phpt':
        "the parent SIGTERMs the SWOOLE_PROCESS server and only then drops client 1, so the "
        "worker's close event for fd-1 races the master freeing its connections at shutdown; "
        'losing it, onClose never logs `close: fd-1` (upstream 27/30, hybrid 23/30, cargo 21/30 '
        'in the aarch64 dev image; closing the client 200 ms before the kill, 20/20, #2018)',
        'tests/swoole_server/discard_timeout_packet.phpt':
        "the client has sent and closed before the server accepts, so the reactor thread "
        "dispatches the first packet and closes the session on the FIN one epoll pass later; "
        "the worker, woken by that packet, discards it only if the close came first. Idle, "
        'upstream 19/19, hybrid 25/31; with a timestamp write around each client step, upstream '
        '9/10 and hybrid 9/10; with a user process polling exist(), upstream 6/10, hybrid 7/10 (#2097)',
        'tests/swoole_server/bug_2639.phpt':
        "onReceive replies, then tasks getClientInfo($fd); the client closes on the reply, and "
        'once the reactor has closed the session getClientInfo is false on every engine (a 5 ms '
        'sleep in onTask: upstream 0/10, hybrid 0/10 arrays). Idle, upstream wins the race in 15 '
        'merge-tier runs and the hybrid loses it in most; under CPU load in the aarch64 dev image, '
        'upstream passes 9/16, hybrid 10/16 (#2198)',
        'tests/swoole_server/max_idle_time_1.phpt':
        'it asserts onClose ran at least 1000 ms after onReceive in whole wall-clock ms, and the '
        "1 s idle timer is scheduled in whole steady ms, so it can land 1 ms short: `Got: 999` on "
        'upstream, cargo and hybrid, once each in 15 merge-tier runs (#2198)',
        'tests/swoole_server/kill_user_process_01.phpt':
        "the same 10 ms restart window as kill_user_process_02, checked by `ps` after SIGTERM: "
        'under CI load both engines miss it (shards failed upstream 4/6, hybrid 4/6 over six '
        'merge-tier runs; locally both pass 30/30 idle and fail 1/15 under load, #1835)',
        'tests/swoole_server/kill_user_process_02.phpt':
        'it asserts the killed user process was restarted within 10 ms; restarts take ~3-4 ms '
        'median on both engines, but the tail passes 10 ms on either (hybrid 12/540, upstream '
        '6/540 over nine aarch64 rounds), more often under CI load (#710)',
        'tests/swoole_thread/server/udp_port.phpt':
        'the first worker to start pushes "begin" to the user process, which echoes it, so the '
        "second worker's \"worker start\" lands after \"begin\" (or after \"done\") whenever that "
        'thread starts later than the push and pop: the same diffs on both engines (ZTS, aarch64 '
        'dev image: upstream 13/29, hybrid 14/28; hybrid 3 of 13 merge-tier runs, #2137)',
        'tests/swoole_thread/server/reset_concurrency.phpt':
        "each of the four worker threads runs `echo 'WORKER STOP', PHP_EOL`, two writes to the "
        'shared stdout, so the lines interleave ("WORKER STOPWORKER STOP") on both engines (zts '
        'merge tier first runs: upstream 143/187, cargo 110/186; dev image upstream 9/15, cargo '
        '12/15, #2739)',
        'tests/swoole_thread/thread_status.phpt':
        "it asserts a new thread isAlive() 10 µs after `new Thread`; whether the OS has run the "
        'thread far enough to mark itself alive by then is the scheduler\'s call, on both engines '
        '(zts merge tier first runs: upstream 6/187, cargo 6/186; dev image upstream 2/15, cargo '
        '2/15, #2739)',
        'tests/swoole_thread/yield.phpt':
        "it asserts a detached thread isAlive() 10 µs after Thread::yield(), which doesn't make "
        'the scheduler run it first (zts merge tier first runs: upstream 2/187, cargo 7/186; dev '
        'image upstream 1/15, cargo 0/15, #2739)',
        'tests/swoole_runtime/file_lock/async_file.phpt':
        "both coroutines open the file on the AIO pool, and whichever open completes first locks "
        "first; when the second's LOCK_SH wins, it holds it for 2 s and the first's < 1 s assert "
        'fails on every engine (upstream 7/26 iouring merge-tier runs and 6/40 in Docker, hybrid '
        '7/40, #862; macOS upstream 5/18, cargo 2/18, #2728)',
        'tests/swoole_runtime/file_hook/async_file.phpt':
        "its 27 AIO reads finish in about 0.8 ms, and the other coroutine's 1 ms usleep fires only "
        "when they cross a millisecond of the timer's whole-millisecond clock, so the count >= 1 "
        'assert fails on both engines (Docker, 260 runs: upstream 125, cargo 143, #2772)',
        'tests/swoole_http_client_coro/slow_server.phpt':
        "the server's body is base64_encode(random_bytes(rand(1024, 65536))), and the client "
        'asserts it is over 5 KiB; base64 of under 3840 bytes is not, so about 4.4% of runs '
        "fail on every engine (#2887's queue run: cargo failed 2 of 3, upstream passed 24)",
        'tests/swoole_server/getClientInfo_in_callback_of_close_2.phpt':
        "the parent kills the SWOOLE_PROCESS server as soon as its client closes, and shutdown "
        "joins the reactor threads, whose destroy callback closes every connection, before it "
        "stops the worker; when that wins, onClose's getClientInfo() finds no session and the "
        'isArray assert fails on both engines (Docker, 150 runs each: upstream 4, cargo 5, #2890)',
        'tests/swoole_coroutine_lock/lock.phpt':
        "its three concurrent requests land on different SWOOLE_PROCESS workers, and when request "
        "2's worker takes the shared lock before request 1's, the output is lock2, value 3, lock1 "
        'on both engines (Docker, 150 runs each: upstream 20, cargo 19, #2923)',
        'tests/swoole_http_server/sendfile_client_reset.phpt':
        "the server sometimes stops sending the file before the client has read 2 MiB, so the "
        "client's recv() loop asserts on empty reads until the runner's timeout, on both engines "
        '(Docker, 24 runs each: upstream 3, cargo 3, #2815)',
        'tests/swoole_http_server_coro/open_frame.phpt':
        "the handler pushes its three frames and returns without reading the client's push, so "
        "the server closes with unread data and resets the connection; when the client's recv() "
        "meets the reset before the frames, the recv() asserts fail on both "
        'engines (Docker: upstream 7/100, cargo 1/50, #2815; #2950\'s queue run: cargo 1 of 3)',
        'tests/swoole_timer/bug_4794_4.phpt':
        'each side wakes or signals the other process and then prints, so the other one can print '
        'first: the expected 1 before 2, 5 before 6 and 8 before 7 rest on a 25 ms usleep; upstream '
        'printed 2 first in alpine (#1420), cargo 6 before 5 and 7 before 8 in two alpine queue runs, '
        'and the PHPT gate saw one first-run failure on each engine in 258 runs (#3001)'}
# Racy only with --enable-thread-context, where each coroutine runs on its own thread.
THREAD_CONTEXT_RACY = {'tests/swoole_coroutine/cancel/throw_exception.phpt':
                       "co 2's third 0.1 s sleep sometimes wakes before co 1's 0.3 s sleep cancels "
                       'it, printing a third "co 2 running" on both engines with the same diff '
                       '(merge tier: upstream 4 of 7 runs, hybrid 11 of 13, #1864)',
                       'tests/swoole_coroutine/eval.phpt':
                       "two coroutines sleep 0.5 s with equal deadlines; while activation's 1 ms "
                       'keep-alive timer is still queued, its removal moves the second sleep to '
                       "the heap's root, so it wakes first on both engines with the same diff "
                       '(merge tier: upstream failed 4 of 9 re-runs, cargo 10 of 10, #2791)',
                       'tests/swoole_coroutine/stats.phpt':
                       "two coroutines both sleep 0.5 s and assert who wakes first, which varies on "
                       'both engines with the same diff (merge tier: upstream failed 21 of 27 runs, '
                       'hybrid 37 of 40, #1948)'}


# Racy only under ASan and UBSan (tools/asan.py), whose slowdown changes the test's timing.
SANITIZER_RACY = {'tests/swoole_websocket_server/get_large_requests.phpt':
                  "the server's onMessage sleeps 1 ms and the last client asserts the worker that "
                  "answers it peaked above one coroutine; slowed clients send too far apart for "
                  "two frames to overlap in one worker, on both engines (tools/asan.py in Docker, "
                  '12 runs each: upstream 4, cargo 1, all on line 30; Sanitizers runs on main 2 of 4, #2992)'}
# A capability the engine reports, and the originals racy only with it.
CONDITIONALLY_RACY = {'thread_context': THREAD_CONTEXT_RACY, 'sanitizers': SANITIZER_RACY}


def racy(case, metadata):
    for capability, cases in CONDITIONALLY_RACY.items():
        if capability in metadata.get('capabilities', []) and case['path'] in cases:
            return cases[case['path']]
    return RACY.get(case['path'])


def require_free_port(port):
    with socket.socket() as guard:
        guard.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        guard.bind(('127.0.0.1', port))


def batches(cases, isolate=False):
    safe = [case for case in cases if case['path'] in PARALLEL_SAFE]
    return ([safe] if safe and not isolate else []) + [
        [case] for case in cases if isolate or case['path'] not in PARALLEL_SAFE]


def prepare(source, destination, cases, support_files=()):
    actual = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != COMMIT:
        raise ValueError(f'Upstream source is {actual}; expected {COMMIT}')
    for item in support_files:
        path = Path(item['path'])
        if not item['path'] or path.is_absolute() or '..' in path.parts or item['path'].startswith('-') or path == Path('.'):
            raise ValueError(f'Unsafe support file: {item["path"]}')
    paths = ['LICENSE', 'tools/bootstrap.php', 'tests/include', *[c['path'] for c in cases]]
    archive = subprocess.check_output(['git', '--literal-pathspecs', '-C', str(source), 'archive', COMMIT, '--', *paths])
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        for member in bundle:
            if not member.isfile():
                continue
            relative = Path(member.name)
            if relative.is_absolute() or '..' in relative.parts:
                raise ValueError(f'Unsafe archive member: {relative}')
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bundle.extractfile(member).read())
    # Some real fixtures are export-ignore: read exact pinned blobs instead of
    # trusting git archive to include every explicitly requested support file.
    for item in support_files:
        data = subprocess.check_output(['git', '-C', str(source), 'cat-file', 'blob', f'{COMMIT}:{item["path"]}'])
        target = destination / item['path']
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    # phpize copies PHP's official runner from the matching PHP installation.
    # Swoole's custom runner can kill unrelated listeners after bind failures.
    runner = source / 'run-tests.php'
    if not runner.is_file():
        raise ValueError('PHP PHPT runner missing; run tools/build_upstream.py (phpize) first')
    shutil.copy2(runner, destination / 'run-tests.php')
    for case in [*cases, *support_files]:
        digest = hashlib.sha256((destination / case['path']).read_bytes()).hexdigest()
        if digest != case['sha256']:
            raise ValueError(f'Upstream test checksum mismatch: {case["path"]}')
    # The full-suite bootstrap starts a daemon under HOME unrelated to these cases.
    # Adapt only that support setup, never PHPT code, assertions, or expectations.
    bootstrap = destination / 'tests/include/bootstrap.php'
    original = bootstrap.read_text()
    remote_setup = "// Run default remote object server\nswoole_library_set_option('default_remote_object_server_worker_num', 2);\nswoole_init_default_remote_object_server();\n"
    if original.count(remote_setup) != 1:
        raise ValueError('Upstream bootstrap remote setup changed; review adapter')
    bootstrap.write_text(original.replace(remote_setup, '// Remote-object service excluded from this isolated selection.\n'))
    # The httpbin/guzzle/symfony cases need the pinned composer libraries. When the
    # upstream tree has them installed (the phpt job runs `composer install` there from
    # the committed lock), copy that vendor dir in; otherwise fall back to a stub that
    # autoloads only the SwooleTest\ fixtures, so the lib-free cases still run offline.
    vendor = destination / 'tests/include/lib/vendor'
    source_vendor = source / 'tests/include/lib/vendor'
    if source_vendor.is_dir():
        shutil.copytree(source_vendor, vendor)
    else:
        vendor.mkdir(parents=True)
        (vendor / 'autoload.php').write_text(AUTOLOAD)


def run_cases(engine, cases, sandbox, artifacts, timeout, jobs, *, offline=True, runner_ini=(), match_child_php=False, php=None):
    sandbox = sandbox.resolve()
    if php:
        # Upstream's tests/run-tests keeps a case's --EXPECT*_85-- section on PHP >= 8.5 and
        # drops it below; PHP's runner would read it as part of the base expectation. Every
        # engine runs the same PHP, so selecting again is a no-op.
        version = int('%d%02d%02d' % tuple(map(int, re.match(r'(\d+)\.(\d+)\.(\d+)', php).groups())))
        for case in cases:
            path = sandbox / case['path']
            if re.search(rb'^--EXPECT[A-Z]*_85--', original := path.read_bytes(), re.M):
                path.write_bytes(select(original, version))
    artifacts.mkdir(parents=True, exist_ok=True)
    status_file = artifacts / 'status.txt'
    status_file.unlink(missing_ok=True)
    junit = artifacts / 'junit.xml'
    command = engine.command(sandbox / 'run-tests.php', '-q', '-n', '-p', shutil.which(engine.php) or engine.php,
                             '--no-color', '--set-timeout', str(timeout),
                             '--show-out', '--show-diff', '-W', status_file)
    online_flag = False
    if offline:
        command.append('--offline')
    else:
        # PHP runner flags differ by installation; inspect the actual copied runner.
        runner = sandbox / 'run-tests.php'
        online_flag = bool(re.search(r"case\s+['\"]--online['\"]\s*:", runner.read_text()))
        if online_flag:
            command.append('--online')
    if jobs > 1 and len(cases) > 1:
        command += [f'-j{jobs}']
    for extension in (*engine.php_extensions, str(engine.extension)):
        command += ['-d', f'extension={extension}']
    for setting in runner_ini:
        command += ['-d', setting]
    command += ['-d', 'zend.assertions=1', *[str(sandbox / case['path']) for case in cases]]
    env = {**os.environ, 'PHPT': '1', 'NO_INTERACTION': '1', 'REPORT_EXIT_STATUS': '1',
           'TEST_PHP_EXECUTABLE': shutil.which(engine.php) or engine.php,
           'TEST_PHP_JUNIT': str(junit), 'HOME': str(sandbox / 'home'), 'TMPDIR': str(sandbox / 'tmp')}
    # Children a test starts as TEST_PHP_EXECUTABLE without -n (start_server, ulimit) read the
    # system ini, which upstream's CI installs the extension into; ours doesn't. A blank entry
    # keeps PHP's compiled scan directory. Runs under -n, the cases themselves, ignore it.
    (scan := artifacts.resolve() / 'php.d').mkdir(exist_ok=True)
    (scan / 'swoole.ini').write_text(f'extension={Path(engine.extension).resolve()}\n')
    env['PHP_INI_SCAN_DIR'] = os.environ.get('PHP_INI_SCAN_DIR', '') + os.pathsep + str(scan)
    if not offline:
        env['SKIP_ONLINE_TESTS'] = '0'
    if all(case['path'] in OUTSIDE_CI for case in cases):
        env.pop('GITHUB_ACTIONS', None)
    child_selector = None
    if match_child_php:
        php = shutil.which(engine.php)
        if not php:
            raise ValueError('Cannot resolve child PHP executable: ' + engine.php)
        argv = [php, *engine.command()[1:]]
        for setting in runner_ini:
            argv += ['-d', setting]
        argv += ['-d', 'zend.assertions=1']
        directory = Path(tempfile.mkdtemp(prefix='child-php-', dir=artifacts.resolve()))
        wrapper = directory / 'php'
        selector = ('import os, shlex, sys\n'
                    f'php = {php!r}\nfallback = {argv[1:]!r}\n'
                    'extra = os.environ.get("TEST_PHP_EXTRA_ARGS")\n'
                    'options = shlex.split(extra) if extra is not None else fallback\n'
                    'os.execv(php, [php, *options, *sys.argv[1:]])\n')
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.join([sys.executable, '-c', selector]) + ' "$@"\n')
        wrapper.chmod(0o755)
        command[0] = php  # The official runner itself must not recurse through the selector.
        env['PATH'] = str(directory) + os.pathsep + env.get('PATH', '')
        child_selector = {'path': str(wrapper), 'sha256': hashlib.sha256(wrapper.read_bytes()).hexdigest(),
                          'fallback_exec_argv': argv, 'argument_policy': 'Use official TEST_PHP_EXTRA_ARGS when present, parsed as argv without shell evaluation; otherwise engine defaults', 'scope': 'Explicit bare php children only; absolute PHP paths are unchanged'}
    started = time.monotonic()
    try:
        result = run_isolated(command, cwd=sandbox, env=env,
                              timeout=((len(cases) + jobs - 1) // jobs) * (timeout + 5) + 15)
        output = result.stdout + result.stderr
        exit_code = result.returncode
    except subprocess.TimeoutExpired as error:
        output = error.stdout or ''
        if isinstance(output, bytes):
            output = output.decode(errors='replace')
        output += '\nOfficial runner exceeded its isolated group deadline'
        exit_code = 124
    runner_seconds = round(time.monotonic() - started, 4)
    receipts_started = time.monotonic()
    (artifacts / 'runner.log').write_text(output)
    expected = {str(sandbox / case['path']): case['path'] for case in cases}
    statuses = {}
    invalid = False
    for line in status_file.read_text().splitlines() if status_file.exists() else []:
        fields = line.split(maxsplit=1)
        if len(fields) != 2 or fields[1] not in expected or fields[1] in statuses:
            invalid = True
        else:
            statuses[fields[1]] = fields[0]
    times = {}
    if junit.exists():
        try:
            times = {node.attrib['name'].split(' (', 1)[0]: float(node.attrib['time'])
                     for node in ET.parse(junit).iter('testcase')}
        except (ET.ParseError, KeyError, ValueError):
            pass
    rows = {}
    for case in cases:
        path = case['path']
        upstream_status = statuses.get(str(sandbox / path), 'MISSING')
        status = {'PASSED': 'passed', 'SKIPPED': 'skipped'}.get(upstream_status, 'failed')
        # A normal batch failure may coexist with genuine passes. An unexplained
        # runner error, duplicate or foreign ledger result can never bless a pass.
        if invalid or exit_code and all(value == 'PASSED' for value in statuses.values()):
            status = 'failed'
        artifact = artifacts / Path(path).with_suffix('')
        artifact.mkdir(parents=True, exist_ok=True)
        for suffix in ['out', 'exp', 'diff', 'log', 'php', 'sh']:
            generated = (sandbox / path).with_suffix('.' + suffix)
            if generated.is_file():
                shutil.copy2(generated, artifact / generated.name)
        row = {'status': status, 'upstream_status': upstream_status, 'exit_code': exit_code,
               'artifacts': str(artifact), 'runner_log': str(artifacts / 'runner.log'),
               'seconds': round(time.monotonic() - started, 4) if len(cases) == 1 else times.get(path),
               'timing_basis': 'isolated_wall_clock' if len(cases) == 1 else 'official_runner_test_execution',
               'official_runner_test_seconds': times.get(path),
               'isolated_runner_seconds': runner_seconds if len(cases) == 1 else None}
        if status != 'passed':
            diff = artifact / Path(path).with_suffix('.diff').name
            row['detail'] = ('Invalid/duplicate result ledger\n' if invalid else '') + (diff.read_text(errors='replace') if diff.exists() else output)[-6000:]
        rows[path] = row
    return rows, {'child_php_selector': child_selector, 'runner_ini': list(runner_ini), 'offline': offline, 'online_flag_supported': online_flag if not offline else None, 'cases': [case['path'] for case in cases], 'jobs': jobs if len(cases) > 1 else 1,
                  'seconds': round(time.monotonic() - started, 4), 'exit_code': exit_code,
                  'timings_seconds': {'runner': runner_seconds,
                                      'receipts_and_artifacts': round(time.monotonic() - receipts_started, 4)}}


def save_report(path, report):
    """Publish checkpoints atomically so concurrent readers never see half a JSON."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=path.name + '.', delete=False) as handle:
        temporary = Path(handle.name)
    try:
        write_report(temporary, report)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)


def record_result(report, engine, case, result):
    row = {'engine': engine.name, 'test': case['path'], 'module': case['module'], 'sha256': case['sha256'], **result}
    report['results'].append(row)
    print(f'{engine.name:10} {case["path"]:65} {row["status"]}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension; repeat to compare')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--manifest', type=Path, default=Path(__file__).with_name('manifest.json'),
                        help='Pinned original-test selection; assertions and capability checks remain unchanged')
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src')
    parser.add_argument('--module', action='append', help='Only select this migration subsystem; repeat as needed')
    parser.add_argument('--case', action='append', help='Exact registered PHPT path; repeatable')
    parser.add_argument('--jobs', type=int, default=1, help='Official runner workers for explicitly audited independent cases')
    parser.add_argument('--isolate-cases', action='store_true', help='Original one-runner/process-group-per-case mode')
    parser.add_argument('--require-all', action='store_true', help='Also fail on unsupported or upstream-skipped cases (except RACY ones)')
    parser.add_argument('--external-network', action='store_true', help='Include cases marked network (DNS/routes) and run every case online')
    parser.add_argument('--match-child-php', action='store_true', help='Explicitly match bare php child commands to this engine; absolute PHP paths are unchanged')
    parser.add_argument('--timeout', type=int, default=15)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/upstream-phpt.json')
    args = parser.parse_args()
    if args.timeout < 1 or args.jobs < 1:
        parser.error('--timeout and --jobs must be positive')
    manifest_bytes = args.manifest.read_bytes()
    manifest = json.loads(manifest_bytes)
    support_files = manifest.get('support_files', [])
    unknown = set(args.module or []) - {case['module'] for case in manifest['tests']}
    if unknown:
        parser.error(f'Unknown modules: {", ".join(sorted(unknown))}')
    cases = [case for case in manifest['tests'] if not args.module or case['module'] in args.module]
    unknown_cases = set(args.case or []) - {case['path'] for case in cases}
    if unknown_cases:
        parser.error(f'Unknown or unselected PHPT paths: {sorted(unknown_cases)}')
    cases = [case for case in cases if not args.case or case['path'] in args.case]
    unmeasured = [case['path'] for case in cases if case.get('network') and not args.external_network]
    cases = [case for case in cases if case['path'] not in unmeasured]
    if not cases:
        parser.error('No tests selected; network cases require --external-network')
    if manifest['commit'] != COMMIT:
        parser.error('PHPT manifest and reference build pins differ')
    if not args.source.is_dir():
        parser.error('Pinned upstream source missing; run python3 tools/build_upstream.py first')
    engines = parse_engines(args.engine, args.php, args.php_extension)
    report = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream': {'version': VERSION, 'commit': COMMIT, 'manifest': str(args.manifest.resolve()), 'manifest_sha256': hashlib.sha256(manifest_bytes).hexdigest(), 'support_files': support_files}, 'engines': [], 'results': [], 'batches': [], 'engine_timings': [], 'timings_seconds': {}, 'unmeasured_cases': unmeasured, 'execution_status': 'running'}
    try:
        save_report(args.output, report)
        with tempfile.TemporaryDirectory(prefix='swoole-phpt-') as temporary:
            sandbox = Path(temporary)
            with timed_phase(report, 'prepare'):
                prepare(args.source, sandbox, cases, support_files=support_files)
                (sandbox / 'home').mkdir()
                (sandbox / 'tmp').mkdir()
                report['upstream']['support_tree'] = subprocess.check_output(['git', '-C', str(args.source), 'rev-parse', f'{COMMIT}:tests/include'], text=True).strip()
                report['upstream']['support_adapter'] = 'Remove unrelated remote-object daemon setup; local SwooleTest PSR-4 autoload only'
                report['upstream']['effective_bootstrap_sha256'] = hashlib.sha256((sandbox / 'tests/include/bootstrap.php').read_bytes()).hexdigest()
                report['upstream']['runner_origin'] = 'PHP official runner supplied by phpize; not Swoole custom tests/run-tests'
                report['upstream']['runner_sha256'] = hashlib.sha256((sandbox / 'run-tests.php').read_bytes()).hexdigest()
            for engine in engines:
                engine_timing = {'engine': engine.name, 'timings_seconds': {}}
                report['engine_timings'].append(engine_timing)
                with timed_phase(engine_timing, 'metadata'):
                    metadata = engine.metadata()
                report['engines'].append(metadata)
                with timed_phase(engine_timing, 'capability_selection'):
                    runnable = []
                    for case in cases:
                        missing = sorted(set(case['requires']) - set(metadata['capabilities']))
                        exclusion = named_exclusion(case, metadata)
                        if exclusion:
                            record_result(report, engine, case, {'status': 'excluded',
                                          'detail': exclusion, 'seconds': 0})
                        elif reason := racy(case, metadata):
                            record_result(report, engine, case, {'status': 'unsupported',
                                          'detail': f'racy original: {reason}', 'seconds': 0})
                        elif missing:
                            record_result(report, engine, case, {'status': 'unsupported',
                                          'detail': f'missing capabilities: {", ".join(missing)}', 'seconds': 0})
                        else:
                            runnable.append(case)
                save_report(args.output, report)
                for group in batches(runnable, args.isolate_cases):
                    if len(group) == 1 and group[0]['path'] in FIXED_PORTS:
                        try:
                            require_free_port(FIXED_PORTS[group[0]['path']])
                        except OSError as error:
                            record_result(report, engine, group[0], {'status': 'failed', 'seconds': 0,
                                          'detail': f'Fixed port {FIXED_PORTS[group[0]["path"]]} unavailable: {error}'})
                            continue
                    artifact = args.output.resolve().parent / (args.output.stem + '-artifacts') / engine.name / f'batch-{len(report["batches"]):03}'
                    with timed_phase(engine_timing, 'cases'):
                        results, timing = run_cases(engine, group, sandbox, artifact, args.timeout, args.jobs, php=metadata.get('php'),
                                                    offline=not (args.external_network or any(case.get('network') for case in group)),
                                                    **({'match_child_php': True} if args.match_child_php else {}))
                    report['batches'].append({'engine': engine.name, **timing})
                    for case in group:
                        record_result(report, engine, case, results[case['path']])
                    save_report(args.output, report)
        report['execution_status'] = 'complete'
    finally:
        if report['execution_status'] == 'running':
            report['execution_status'] = 'interrupted'
        save_report(args.output, report)
    # A RACY row stays unsupported in the report; its verdict means nothing on either engine.
    return int(any(verdict_failed(row, args.require_all) for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
