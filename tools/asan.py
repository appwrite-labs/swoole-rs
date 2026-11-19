#!/usr/bin/env python3
"""Swoole under AddressSanitizer and UndefinedBehaviorSanitizer (#1899).

PHP, the extension's C/C++ (upstream sources and the native/ shims, which the crates'
build scripts compile with CC and CFLAGS) and the Rust crates are all instrumented:
clang's -fsanitize=address,undefined and rustc's -Zsanitizer=address call into one
runtime, the one clang links into the PHP executable, so every object in the process
reports through it. dev.py integrate runs every module gate, the application corpus among
them; then tools/build_ext.py builds the Cargo swoole.so with the same flags and the PHPT
selection runs on it. The native gates and the crates' unit tests link the instrumented Rust archive into
executables of their own: target/asan/bin puts `cc` and `c++` first on PATH as clang with
the sanitizers, so every link takes clang's runtime, and -Zexternal-clangrt stops rustc
adding its own beside it; the gates that call $CC and $CXX get the same shims. Rust
globals carry no redzones (-asan-globals=0): rustc registers each one from a section GNU
ld keeps whole, so under --gc-sections every global and every function a global's table
points to stays, and the gates' executables would need all the extension's libraries
(OpenSSL, the multipart parser, the context switch) to link. C and C++ globals keep
theirs. PHP has FFI built in for the timer and ports gates. A report from code this repository
builds fails the run whatever the tests say; upstream's reference build is counted apart. Reports stay in target/asan/reports/ and are only counted on stdout: a
report may locate an upstream bug, which goes to the private security repository (AGENTS.md), so a public CI log
never shows one. Rerun locally to read them. The loader runs LeakSanitizer at MSHUTDOWN.
A process-mode worker checks separately at worker stop after serving a request.
Workers exit without shutting PHP down. Forked PHP processes that reach MSHUTDOWN
skip the check there because that shutdown path hung in #2345. tools/lsan.supp names PHP's own allocations that
look lost there. USE_TRACKED_ALLOC hands the request heap to malloc, as PHP's CI runs
it. PCRE's JIT is off, its SIMD scans read past the subject.

    python3 tools/dev.py docker -- tools/asan.py
"""
import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT, run_isolated
OUT = ROOT / 'target/asan'
# vptr checks need the C++ UBSan runtime, which a C program like PHP doesn't link.
SANITIZE = '-fsanitize=address,undefined -fno-sanitize=vptr -fno-omit-frame-pointer'


def compilers(shims):
    """One runtime and locale-correct, instrumented case comparisons at every link."""
    shims.mkdir(parents=True, exist_ok=True)
    case = shims / 'case.o'
    subprocess.run(['clang', *SANITIZE.split(), '-fPIC', '-fno-builtin', '-c',
                    ROOT / 'tools/asan_case.c', '-o', case], check=True)
    for name, compiler in [('cc', 'clang'), ('c++', 'clang++')]:
        command = f'{compiler} {SANITIZE} "$@" -fno-sanitize=vptr'
        # Probes/compilation must not link; libtool may already pass our helper back.
        (shims / name).write_text(f'''#!/bin/sh
case "$*" in --version|-v|-V|-dump*|-print*) exec {command};; esac
for arg; do
    case "$arg" in -c|-E|-S|{shlex.quote(str(case))}) exec {command};; esac
done
exec {command} {shlex.quote(str(case))}
''')
        (shims / name).chmod(0o755)
    return {**os.environ, 'CC': str(shims / 'cc'), 'CXX': str(shims / 'c++'),
            'PATH': f"{shims}{os.pathsep}{os.environ['PATH']}"}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--jobs', type=int, default=min(os.cpu_count() or 2, 4))
    parser.add_argument('--timeout', type=int, default=120, help='Per PHPT, in seconds')
    parser.add_argument('--gate', action='append', default=[], help='Only this gate phase; repeatable')
    parser.add_argument('--case', action='append', default=[], help='Only this PHPT (run.py --case); repeatable')
    args = parser.parse_args()
    php = OUT / 'php'
    # One link recipe for every executable the run builds, whatever compiler name it calls.
    # A gate's own flags follow, so its -fno-sanitize= choices hold (the WebSocket codec's
    # pinned C++ reads unaligned); vptr stays off whatever it asks.
    compiler_env = compilers(OUT / 'bin')
    subprocess.run([ROOT / '.devcontainer/php.sh', php, '--enable-address-sanitizer', '--enable-undefined-sanitizer',
                    '--without-pcre-jit', '--with-pdo-sqlite', '--with-ffi'],
                   env=compiler_env, check=True)
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    build = {**compiler_env, 'CFLAGS': SANITIZE, 'CXXFLAGS': SANITIZE, 'LDFLAGS': SANITIZE, 'RUSTC_BOOTSTRAP': '1',
             'RUSTFLAGS': '-Zsanitizer=address -Zexternal-clangrt=yes -Cforce-frame-pointers=yes -Cllvm-args=-asan-globals=0',
             # As `cargo --target`: build scripts and proc macros, which rustc loads, stay uninstrumented.
             'CARGO_BUILD_TARGET': host}
    reports = OUT / 'reports'
    shutil.rmtree(reports, ignore_errors=True)
    reports.mkdir(parents=True)
    options = f'log_path={reports}/report:print_stacktrace=1'
    leaks = f':leak_check_at_exit=0:suppressions={ROOT}/tools/lsan.supp:print_suppressions=0'
    run = {**build,
           'PATH': f"{php / 'bin'}{os.pathsep}{build['PATH']}", 'USE_ZEND_ALLOC': '0',
           'USE_TRACKED_ALLOC': '1', 'ASAN_OPTIONS': options, 'LSAN_OPTIONS': options + leaks,
           'UBSAN_OPTIONS': options}
    gates = [sys.executable, 'tools/dev.py', 'integrate', '--all-modules', '--php', str(php / 'bin/php')]
    # Every gate but the lints, cargo_asan.py's runs (they make an ASan build of their own) and the
    # syscall differential: ASan's runtime makes syscalls of its own, and `applications` already
    # runs the same projects under ASan.
    planned = json.loads(subprocess.check_output([*gates, '--plan'], cwd=ROOT, env=run, text=True))
    phases = [arg for entry in planned if entry['phase'] not in {'format', 'clippy', 'ownership', 'syscalls'}
              and not entry['phase'].endswith('-rust-asan') for arg in ['--phase', entry['phase']]]
    # --gate and --case narrow the run to what they name; naming only one half skips the other.
    if args.gate or args.case:
        phases = [part for phase in args.gate for part in ('--phase', phase)]
    cases = [part for case in args.case for part in ('--case', case)]
    failed = []
    if (phases or not args.case) and subprocess.run([*gates, *phases, '--with-prerequisites', '--jobs', str(args.jobs),
                       '--phase-timeout', '2400'], cwd=ROOT, env=run).returncode:
        failed.append('gates')
    if cases or not args.gate:
        if subprocess.run([sys.executable, 'tools/build_ext.py', '--php-config', php / 'bin/php-config'], cwd=ROOT, env=run).returncode:
            failed.append('extension build')
        else:
            if not worker_leak_check(php / 'bin/php', run, args.timeout):
                failed.append('worker leak check')
            if subprocess.run([sys.executable, 'corpus/upstream/run.py', '--timeout', str(args.timeout), '--php', php / 'bin/php',
                               '--engine', f"cargo={ROOT / 'target/ext/swoole.so'}", '--output', OUT / 'phpt.json', *cases], cwd=ROOT, env=run).returncode:
                failed.append('phpt')
    kinds = [origin(path.read_text(errors='replace')) for path in sorted(reports.iterdir())]
    if found := kinds.count('ours'):
        failed.append(f'{found} sanitizer reports in {reports}')
    if upstream := kinds.count('upstream'):
        print(f"{upstream} reports from upstream's reference build in {reports}: triage them privately")
    print('sanitizer run: ' + (', '.join(failed) + ' failed' if failed else 'clean'))
    return int(bool(failed))


def worker_leak_check(php, env, timeout):
    # A worker exits independently of the master. Require its own success marker.
    try:
        result = run_isolated([php, '-n', '-d', 'ffi.enable=1', '-d', 'swoole.enable_library=0',
                               '-d', f'extension={ROOT}/target/ext/swoole.so',
                               ROOT / 'tests/modules/integration/worker_leaks.php'], env=env, timeout=timeout)
    except subprocess.TimeoutExpired:
        return False
    return result.returncode == 0 and result.stdout.splitlines() == ['worker leak check passed']


def origin(report):
    """'upstream' when the first repository code a report names is upstream's reference build
    (target/upstream, which swoole.so never compiles), 'notice' for a file with no error in it
    (ASan's note on a coroutine stack switch), else 'ours'. PHP's own frames are skipped."""
    if 'ERROR: ' not in report and 'runtime error' not in report:
        return 'notice'
    for line in report.splitlines():
        for path in line.split(str(ROOT) + '/')[1:]:
            if not path.startswith('target/asan/php/'):
                return 'upstream' if path.startswith('target/upstream/') else 'ours'
    return 'ours'


if __name__ == '__main__':
    sys.exit(main())
