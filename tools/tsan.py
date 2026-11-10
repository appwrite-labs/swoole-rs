#!/usr/bin/env python3
"""The ZTS lane under ThreadSanitizer (#2252): data races in the state Swoole\\Thread's
Map, ArrayList, Queue, Atomic, Lock and Barrier share across threads.

A thread-safe PHP, the pinned upstream and the Cargo-built swoole.so, all built with
--enable-swoole-thread and instrumented: clang's -fsanitize=thread for PHP and the C/C++
(upstream's, and native/ through the crates' build scripts), rustc's -Zsanitizer=thread for
the crates and, through build-std, the standard library, whose atomics TSan must see. One
runtime serves them all, the one clang links into the PHP executable. Then the
swoole_thread, swoole_atomic and swoole_lock PHPT families run on both engines, and
exit_guard.py on the pair.

First a planted race: tools/tsan_canary.rs, built with the crates' rustc flags and preloaded into
this PHP, must be reported, or the run proves nothing. Then any report fails the run, on
either engine. A race upstream has too goes in tools/tsan.supp by name, never by a blanket
rule; one only the Cargo build has is a p0 kind:bug. Reports stay in target/tsan/reports/
and are only counted on stdout: a report may locate an upstream bug, which goes to the private
security repository (AGENTS.md), so a public CI log never shows one. Rerun locally to read
them. Needs clang with its TSan runtime and a kernel with at most 28 bits of mmap
randomisation (vm.mmap_rnd_bits; tsan.yml sets it).

    python3 tools/dev.py docker -- tools/tsan.py
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'target/tsan'
SANITIZE = '-fsanitize=thread -fno-omit-frame-pointer'
FAMILIES = ['swoole_thread', 'swoole_atomic', 'swoole_lock']


def sanitized(reports, name):
    """TSan's options for one run's processes, reporting to files under reports/name."""
    (reports / name).mkdir(parents=True)
    # A worker forked from a threaded process may start threads; TSan would kill it.
    options = (f'log_path={reports / name}/report:suppressions={ROOT}/tools/tsan.supp:'
               'print_suppressions=0:die_after_fork=0:second_deadlock_stack=1')
    return {'TSAN_OPTIONS': options}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--timeout', type=int, default=120, help='Per PHPT, in seconds')
    args = parser.parse_args()
    php = OUT / 'php'
    compilers = {**os.environ, 'CC': 'clang', 'CXX': 'clang++', 'CFLAGS': SANITIZE, 'CXXFLAGS': SANITIZE,
                 'LDFLAGS': '-fsanitize=thread'}
    # PHP dlopens extensions with RTLD_DEEPBIND unless ASan or MSan is on, and TSan refuses that.
    # zend_portability.h prefers the RTLD_GROUP branch, which glibc lacks: defining its flags as
    # 0 leaves a plain dlopen(RTLD_GLOBAL).
    deepbind = ' -DRTLD_GROUP=0 -DRTLD_WORLD=0 -DRTLD_PARENT=0'
    subprocess.run([ROOT / '.devcontainer/php.sh', php, '--enable-zts'],
                   env={**compilers, 'CFLAGS': SANITIZE + deepbind}, check=True)
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    subprocess.run(['rustup', 'component', 'add', 'rust-src'], cwd=ROOT, check=True)
    rust = {'RUSTC_BOOTSTRAP': '1', 'RUSTFLAGS': '-Zsanitizer=thread -Zexternal-clangrt=yes -Cforce-frame-pointers=yes'}
    # As `cargo --target`: build scripts and proc macros, which rustc loads, stay uninstrumented.
    build = {**compilers, **rust, 'CARGO_BUILD_TARGET': host, 'CARGO_UNSTABLE_BUILD_STD': 'std,panic_abort'}
    config = ['--php-config', str(php / 'bin/php-config'), '--configure=--enable-swoole-thread']
    # A configured build gets its own tree, rebuilt when the compiler flags change; it prints its library.
    built = subprocess.run([sys.executable, 'tools/build_upstream.py', *config, '--phpize', str(php / 'bin/phpize')],
                           cwd=ROOT, env=build, check=True, stdout=subprocess.PIPE, text=True).stdout
    print(built, end='')
    upstream = Path(next(line.split(': ', 1)[1] for line in built.splitlines()
                         if line.startswith(('Built upstream: ', 'Cached upstream: '))))
    subprocess.run([sys.executable, 'tools/build_ext.py', *config], cwd=ROOT, env=build, check=True)
    engines = {'upstream': OUT / 'upstream.so', 'cargo': OUT / 'cargo.so'}
    shutil.copy2(upstream, engines['upstream'])
    shutil.copy2(ROOT / 'target/ext/swoole.so', engines['cargo'])
    reports = OUT / 'reports'
    shutil.rmtree(reports, ignore_errors=True)
    shutil.rmtree(OUT / 'phpt', ignore_errors=True)
    (OUT / 'phpt').mkdir(parents=True)
    run = {**os.environ, 'USE_ZEND_ALLOC': '0', 'PATH': f"{php / 'bin'}{os.pathsep}{os.environ['PATH']}"}

    canary = OUT / 'canary.so'
    # Against the prebuilt, uninstrumented std: the race is in the canary's own code.
    subprocess.run(['rustc', '--edition=2024', '--crate-type=cdylib', *rust['RUSTFLAGS'].split(),
                    '-Cunsafe-allow-abi-mismatch=sanitizer', '-o', canary,
                    ROOT / 'tools/tsan_canary.rs'], env={**os.environ, **rust}, check=True)
    subprocess.run([php / 'bin/php', '-n', '-r', ''], env={**run, **sanitized(reports, 'canary'), 'LD_PRELOAD': str(canary)})
    if not any((reports / 'canary').iterdir()):
        print('sanitizer run: the planted race went unreported')
        return 1
    shutil.rmtree(reports / 'canary')

    failed = []
    subprocess.run([sys.executable, 'corpus/upstream/inventory.py', '--output', OUT / 'phpt/manifest.json'], cwd=ROOT, check=True)
    for name, extension in engines.items():
        families = [arg for family in FAMILIES for arg in ['--module', family]]
        status = subprocess.run([sys.executable, 'corpus/upstream/run.py', '--manifest', OUT / 'phpt/manifest.json',
                                 *families, '--timeout', str(args.timeout), '--php', php / 'bin/php',
                                 '--engine', f'{name}={extension}', '--output', OUT / f'phpt/{name}.json'],
                                cwd=ROOT, env={**run, **sanitized(reports, name)}).returncode
        if status not in (0, 1):  # 1: a test failed, which the ZTS lane gates
            failed.append(f'{name} phpt')
    if subprocess.run([sys.executable, 'tests/modules/phphelpers/exit_guard.py', '--php', php / 'bin/php',
                       '--php-config', php / 'bin/php-config', '--build-manifest', ROOT / 'target/ext/build-manifest.json',
                       *[arg for name, extension in engines.items() for arg in ['--engine', f'{name}={extension}']],
                       '--output', OUT / 'phpt/guard.json'], cwd=ROOT, env={**run, **sanitized(reports, 'guard')}).returncode:
        failed.append('exit_guard')
    for run_name in sorted(path.name for path in reports.iterdir()):
        if found := len(list((reports / run_name).iterdir())):
            failed.append(f'{found} sanitizer reports in {reports / run_name}')
    print('sanitizer run: ' + (', '.join(failed) + ' failed' if failed else 'clean'))
    return int(bool(failed))


if __name__ == '__main__':
    sys.exit(main())
