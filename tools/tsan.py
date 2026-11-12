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
this PHP, must be reported, or the run proves nothing. Upstream is the reference, so its
reports are counted and never fail the run. Every other report is a race in this repository's
code, fixed rather than suppressed: the run fails while there are more than LIMIT, which falls
as fixes land, the way the callgrind records do, until it reaches zero (#2252). Reports stay in
target/tsan/reports/ and are only counted on stdout: a report may locate an upstream bug, which
goes to the private security repository (AGENTS.md), so a public CI log never shows one. Rerun
locally to read them. Needs clang with its TSan runtime and a kernel with at most 28 bits of mmap
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
# Reports outside upstream's run that main's run has. Lower it with each fix. 18 locally once the
# master pid and running flag stopped racing (#3343), plus 5 for the runs' swing in reports that only
# hold TSan's "can't find longjmp buf" (1 on main's local run, 6 on the next).
LIMIT = 23


def sanitized(reports, name):
    """TSan's options for one run's processes, reporting to files under reports/name."""
    (reports / name).mkdir(parents=True)
    # A worker forked from a threaded process may start threads; TSan would kill it.
    options = f'log_path={reports / name}/report:die_after_fork=0:second_deadlock_stack=1'
    return {'TSAN_OPTIONS': options}


def outside_upstream(counts):
    """The reports LIMIT bounds: those in every directory but upstream's runs'."""
    return sum(found for name, found in counts.items() if not name.startswith('upstream'))


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
    # PCRE's JIT is off, as in tools/asan.py: its generated code reports races that have no
    # frame of Swoole's or this repository's, in whichever PHPTs reach preg_* (#3344).
    subprocess.run([ROOT / '.devcontainer/php.sh', php, '--enable-zts', '--without-pcre-jit'],
                   env={**compilers, 'CFLAGS': SANITIZE + deepbind}, check=True)
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    subprocess.run(['rustup', 'component', 'add', 'rust-src'], cwd=ROOT, check=True)
    # cfg(tsan) gives each coroutine a TSan fiber of its own (crates/coroutine/src/context.rs).
    rust = {'RUSTC_BOOTSTRAP': '1', 'RUSTFLAGS': '-Zsanitizer=thread -Zexternal-clangrt=yes -Cforce-frame-pointers=yes --cfg tsan'}
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
    # exit_guard.py needs the pair in one run through one PHP: a wrapper sends each engine's
    # reports to its own directory, by the extension it loads.
    options = {name: sanitized(reports, f'{name}-guard')['TSAN_OPTIONS'] for name in engines}
    guard = OUT / 'guard-php'
    guard.write_text(f"""#!/bin/sh
case "$*" in *'extension={engines['upstream']}'*) TSAN_OPTIONS='{options['upstream']}' ;; *) TSAN_OPTIONS='{options['cargo']}' ;; esac
export TSAN_OPTIONS
exec '{php / 'bin/php'}' "$@"
""")
    guard.chmod(0o755)
    if subprocess.run([sys.executable, 'tests/modules/phphelpers/exit_guard.py', '--php', guard,
                       '--php-config', php / 'bin/php-config', '--build-manifest', ROOT / 'target/ext/build-manifest.json',
                       *[arg for name, extension in engines.items() for arg in ['--engine', f'{name}={extension}']],
                       '--output', OUT / 'phpt/guard.json'], cwd=ROOT, env=run).returncode:
        failed.append('exit_guard')
    counts = {path.name: len(list(path.iterdir())) for path in sorted(reports.iterdir())}
    ours = outside_upstream(counts)
    print('sanitizer reports: ' + ', '.join(f'{found} in {reports / name}' for name, found in counts.items()))
    if ours > LIMIT:
        failed.append(f'{ours} reports outside upstream, over the limit of {LIMIT},')
    elif ours < LIMIT:
        print(f'{ours} reports outside upstream: lower LIMIT in tools/tsan.py to {ours}')
    print('sanitizer run: ' + (', '.join(failed) + ' failed' if failed else 'clean'))
    return int(bool(failed))


if __name__ == '__main__':
    sys.exit(main())
