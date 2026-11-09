#!/usr/bin/env python3
"""The all-modules hybrid under AddressSanitizer and UndefinedBehaviorSanitizer (#1899).

PHP, the extension's C/C++ (upstream sources and the native/ shims, which the crates'
build scripts compile with CC and CFLAGS) and the Rust crates are all instrumented:
clang's -fsanitize=address,undefined and rustc's -Zsanitizer=address call into one
runtime, the one clang links into the PHP executable, so every object in the process
reports through it. dev.py integrate builds it (and the upstream reference beside it) and
runs every module gate that loads an extension into PHP, the application corpus among
them; then the PHPT selection runs on it. Any sanitizer report fails the run whatever
the tests say. Reports stay in target/asan/reports/ and are only counted on stdout: a report may locate an upstream
bug, which goes to the private security repository (AGENTS.md), so a public CI log
never shows one. Rerun locally to read them. Leak checking is off (PHP frees nothing
at exit by design), and so is PCRE's JIT, whose SIMD scans read past the subject.

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
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import build_alias  # noqa: E402

OUT = ROOT / 'target/asan'
# vptr checks need the C++ UBSan runtime, which a C program like PHP doesn't link.
SANITIZE = '-fsanitize=address,undefined -fno-sanitize=vptr -fno-omit-frame-pointer'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--jobs', type=int, default=min(os.cpu_count() or 2, 4))
    parser.add_argument('--timeout', type=int, default=120, help='Per PHPT, in seconds')
    args = parser.parse_args()
    php = OUT / 'php'
    compilers = {**os.environ, 'CC': 'clang', 'CXX': 'clang++'}
    subprocess.run([ROOT / '.devcontainer/php.sh', php, '--enable-address-sanitizer', '--enable-undefined-sanitizer',
                    '--without-pcre-jit', '--with-pdo-sqlite'],
                   env=compilers, check=True)
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    build = {**compilers, 'CFLAGS': SANITIZE, 'CXXFLAGS': SANITIZE, 'LDFLAGS': SANITIZE, 'RUSTC_BOOTSTRAP': '1',
             'RUSTFLAGS': '-Zsanitizer=address -Cforce-frame-pointers=yes',
             # As `cargo --target`: build scripts and proc macros, which rustc loads, stay uninstrumented.
             'CARGO_BUILD_TARGET': host}
    reports = OUT / 'reports'
    shutil.rmtree(reports, ignore_errors=True)
    reports.mkdir(parents=True)
    options = f'log_path={reports}/report:print_stacktrace=1'
    run = {**build, 'PATH': f"{php / 'bin'}{os.pathsep}{os.environ['PATH']}", 'USE_ZEND_ALLOC': '0',
           'ASAN_OPTIONS': options + ':detect_leaks=0', 'UBSAN_OPTIONS': options}
    gates = [sys.executable, 'tools/dev.py', 'integrate', '--all-modules', '--php', str(php / 'bin/php')]
    # The gates that run PHP with the extension; the rest are native or Rust-only and build their own way.
    planned = json.loads(subprocess.check_output([*gates, '--plan'], cwd=ROOT, env=run, text=True))
    phases = [arg for entry in planned if '--engine' in entry['command'] for arg in ['--phase', entry['phase']]]
    failed = []
    if subprocess.run([*gates, *phases, '--with-prerequisites', '--jobs', str(args.jobs),
                       '--phase-timeout', '2400'], cwd=ROOT, env=run).returncode:
        failed.append('gates')
    extension = ROOT / 'target/hybrid' / build_alias('rust') / 'modules/swoole.so'
    if subprocess.run([sys.executable, 'corpus/upstream/run.py', '--timeout', str(args.timeout), '--php', str(php / 'bin/php'),
                       '--engine', f'hybrid={extension}', '--output', OUT / 'phpt.json'], cwd=ROOT, env=run).returncode:
        failed.append('phpt')
    if found := len(list(reports.iterdir())):
        failed.append(f'{found} sanitizer reports in {reports}')
    print('sanitizer run: ' + (', '.join(failed) + ' failed' if failed else 'clean'))
    return int(bool(failed))


if __name__ == '__main__':
    sys.exit(main())
