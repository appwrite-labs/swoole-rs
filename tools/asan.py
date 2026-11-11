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
theirs. PHP has FFI built in for the timer and ports gates. Any sanitizer report fails
the run whatever the tests say. Reports stay in target/asan/reports/ and are only counted on stdout: a
report may locate an upstream bug, which goes to the private security repository (AGENTS.md), so a public CI log
never shows one. Rerun locally to read them. LeakSanitizer runs at the extension's
MSHUTDOWN, not at exit, and only in the process that loaded the extension: a worker exits
without shutting PHP down, and a forked child's check hangs (#2345). tools/lsan.supp names PHP's own allocations that
look lost there. USE_TRACKED_ALLOC hands the request heap to malloc, as PHP's CI runs
it. PCRE's JIT is off, its SIMD scans read past the subject.

    python3 tools/dev.py docker -- tools/asan.py
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/asan'
# vptr checks need the C++ UBSan runtime, which a C program like PHP doesn't link.
SANITIZE = '-fsanitize=address,undefined -fno-sanitize=vptr -fno-omit-frame-pointer'


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
    shims = OUT / 'bin'
    shims.mkdir(parents=True, exist_ok=True)
    for name, compiler in [('cc', 'clang'), ('c++', 'clang++')]:
        (shims / name).write_text(f'#!/bin/sh\nexec {compiler} {SANITIZE} "$@" -fno-sanitize=vptr\n')
        (shims / name).chmod(0o755)
    compilers = {**os.environ, 'CC': 'clang', 'CXX': 'clang++', 'PATH': f"{shims}{os.pathsep}{os.environ['PATH']}"}
    subprocess.run([ROOT / '.devcontainer/php.sh', php, '--enable-address-sanitizer', '--enable-undefined-sanitizer',
                    '--without-pcre-jit', '--with-pdo-sqlite', '--with-ffi'],
                   env=compilers, check=True)
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    build = {**compilers, 'CFLAGS': SANITIZE, 'CXXFLAGS': SANITIZE, 'LDFLAGS': SANITIZE, 'RUSTC_BOOTSTRAP': '1',
             'RUSTFLAGS': '-Zsanitizer=address -Zexternal-clangrt=yes -Cforce-frame-pointers=yes -Cllvm-args=-asan-globals=0',
             # As `cargo --target`: build scripts and proc macros, which rustc loads, stay uninstrumented.
             'CARGO_BUILD_TARGET': host}
    reports = OUT / 'reports'
    shutil.rmtree(reports, ignore_errors=True)
    reports.mkdir(parents=True)
    options = f'log_path={reports}/report:print_stacktrace=1'
    leaks = f':leak_check_at_exit=0:suppressions={ROOT}/tools/lsan.supp:print_suppressions=0'
    run = {**build, 'CC': str(shims / 'cc'), 'CXX': str(shims / 'c++'),
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
    if (cases or not args.gate) and (
            subprocess.run([sys.executable, 'tools/build_ext.py', '--php-config', php / 'bin/php-config'], cwd=ROOT, env=run).returncode
            or subprocess.run([sys.executable, 'corpus/upstream/run.py', '--timeout', str(args.timeout), '--php', php / 'bin/php',
                               '--engine', f"cargo={ROOT / 'target/ext/swoole.so'}", '--output', OUT / 'phpt.json', *cases],
                              cwd=ROOT, env=run).returncode):
        failed.append('phpt')
    if found := len(list(reports.iterdir())):
        failed.append(f'{found} sanitizer reports in {reports}')
    print('sanitizer run: ' + (', '.join(failed) + ' failed' if failed else 'clean'))
    return int(bool(failed))


if __name__ == '__main__':
    sys.exit(main())
