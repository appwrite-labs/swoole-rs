#!/usr/bin/env python3
"""Run all unchanged original timer GoogleTests against native and Rust queues."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.build_hybrid import overlay_hybrid_tree

from tools.build_upstream import COMMIT as PIN
ORIGINAL = 'core-tests/src/os/timer.cpp'
SUPPORT = ['core-tests/src/main.cpp', 'core-tests/include/test_core.h']


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def put(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)


def stage(source, tree):
    packed = subprocess.check_output(['git', '-C', str(source), 'archive', PIN])
    with tarfile.open(fileobj=io.BytesIO(packed)) as bundle:
        for member in bundle:
            if member.isfile():
                path = Path(member.name)
                if path.is_absolute() or '..' in path.parts:
                    raise ValueError('Unsafe reference archive path')
                put(tree / path, bundle.extractfile(member).read())
    for name in [ORIGINAL, *SUPPORT]:
        put(tree / name, subprocess.check_output(['git', '-C', str(source), 'show', f'{PIN}:{name}']))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True, help='Read-only configured Linux reference config.h')
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test (<build>/rust-prefix/lib/libswoole_rs_ffi.a)')
    parser.add_argument('--build-root', type=Path, required=True, help='Private build/cache directory')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=10)
    parser.add_argument('--sanitize', action='store_true')
    parser.add_argument('--rust-asan', action='store_true', help='Supplied archive was separately compiled with Rust ASan')
    parser.add_argument('--timed-locks', action='store_true', help='Also exercise actual unchanged native Mutex/RWLock timed consumers')
    parser.add_argument('--ucontext-timers', action='store_true', help='Compile native ucontext configuration and exercise live timer/coroutine custody')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux' or args.jobs < 1 or args.timeout < 1:
        parser.error('Linux native build, positive jobs/timeout required')
    source, archive = args.source.resolve(strict=True), args.archive.resolve(strict=True)
    if args.rust_asan:
        if not args.sanitize:
            parser.error('--rust-asan requires --sanitize for its native runtime')
        symbols = subprocess.run(['nm', '-u', str(archive)], capture_output=True, text=True)
        if '__asan_report_' not in symbols.stdout:
            parser.error('Selected archive lacks Rust ASan report imports')
    directory = args.build_root.resolve()
    if directory == source or source in directory.parents or ROOT == directory:
        parser.error('Private build root must be separate from reference and repository root')
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
        'scope': 'Unchanged original timer.cpp and main.cpp with real GoogleTest and full native runtime. Each original runs in its own process. Native ASan/UBSan only when selected; Rust archive uninstrumented. No PHP or performance claim.',
        'native_sanitized': args.sanitize, 'rust_asan_declared': args.rust_asan, 'results': [], 'engines': [], 'passed': False,
        'timed_lock_results': [], 'ucontext_results': [], 'ucontext_requested': args.ucontext_timers}
    if args.rust_asan:
        report['scope'] = 'Unchanged original timer.cpp/main.cpp, real GoogleTest/full native runtime ASan/UBSan and separately instrumented Rust timer ASan. Precompiled Rust standard library uninstrumented. Each original runs in its own process. No PHP/performance claim.'
    environment = dict(os.environ)
    if args.sanitize:
        environment.update(ASAN_OPTIONS='detect_leaks=1:abort_on_error=1', UBSAN_OPTIONS='halt_on_error=1:print_stacktrace=1')
        report['sanitizer_options'] = {key: environment[key] for key in ['ASAN_OPTIONS', 'UBSAN_OPTIONS']}
    try:
        for engine in ['upstream', 'rust']:
            tree = directory / engine
            stage(source, tree)
            inventory = ['timer.' + name for name in re.findall(r'TEST\(timer,\s*(\w+)\)', (tree / ORIGINAL).read_text())]
            if len(inventory) != 9 or len(set(inventory)) != 9:
                raise ValueError('Original nine-test inventory changed')
            report['inventory'] = inventory
            identity = {name: sha(tree / name) for name in [ORIGINAL, *SUPPORT]}
            if engine == 'rust':
                # Every module's patches, as the hybrid build applied them (#796).
                hybrid_cmake = overlay_hybrid_tree(archive, tree)
            config = args.config.read_bytes()
            if args.ucontext_timers:
                if re.search(rb'^#define SW_USE_THREAD_CONTEXT\b', config, re.M):
                    raise ValueError('Thread-context requires its separate ownership contract')
                config = re.sub(rb'^#define SW_USE_ASM_CONTEXT\b[^\n]*', b'#undef SW_USE_ASM_CONTEXT', config, flags=re.M)
            put(tree / 'config.h', config)
            cmake = (tree / 'CMakeLists.txt').read_text()
            selector = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
            if cmake.count(selector) != 1:
                raise ValueError('Original CMake selection changed')
            cmake = cmake.replace(selector, 'set(core_test_files core-tests/src/main.cpp ' + ORIGINAL + ')')
            if engine == 'rust':
                cmake += f'\n{hybrid_cmake}target_link_libraries(lib-swoole "{archive}")\n'
            put(tree / 'CMakeLists.txt', cmake.encode())
            if identity != {name: sha(tree / name) for name in identity}:
                raise ValueError('Original assertion or support source modified')
            metadata = {'name': engine}
            report['engines'].append(metadata)
            configure = ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release']
            flags = '-fsanitize=address,undefined -fno-omit-frame-pointer' if args.sanitize else ''
            configure += ['-DCMAKE_CXX_FLAGS=' + flags, '-DCMAKE_C_FLAGS=' + flags, '-DCMAKE_EXE_LINKER_FLAGS=' + flags, '-DCMAKE_SHARED_LINKER_FLAGS=' + flags]
            for name, command in [('configure', configure), ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'core-tests', '-j' + str(args.jobs)])]:
                result = run_isolated(command, timeout=600)
                log = tree / (name + '.log')
                log.write_text(result.stdout + result.stderr)
                metadata[name] = {'command': command, 'returncode': result.returncode, 'log': str(log)}
                if result.returncode:
                    raise RuntimeError(engine + ' ' + name + ' failed: ' + str(log))
            binary = tree / 'build/bin/core-tests'
            for case in inventory:
                xml = tree / (case + '.xml')
                xml.unlink(missing_ok=True)
                row = {'engine': engine, 'name': case, 'status': 'failed'}
                try:
                    result = run_isolated([str(binary), '--gtest_filter=' + case, '--gtest_output=xml:' + str(xml)], timeout=args.timeout, cwd=tree, env=environment)
                    row.update(returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)
                    if xml.is_file():
                        cases = list(ET.parse(xml).iter('testcase'))
                        if len(cases) != 1 or cases[0].attrib['classname'] + '.' + cases[0].attrib['name'] != case:
                            raise ValueError('Missing, duplicate or foreign original test result')
                        value = cases[0]
                        row['status'] = 'skipped' if value.find('skipped') is not None or value.attrib.get('status') != 'run' else 'passed' if result.returncode == 0 and value.find('failure') is None and value.find('error') is None else 'failed'
                except subprocess.TimeoutExpired as error:
                    row.update(error=str(error), stdout=str(error.stdout), stderr=str(error.stderr))
                report['results'].append(row)
                write_report(args.output, report)
            if args.timed_locks:
                fixture = Path(__file__).with_name('clock_lock.cc')
                consumer = tree / 'build/bin/timer-clock-lock'
                command = ['c++', '-std=c++17', '-O1', '-g', '-DHAVE_CONFIG_H', '-I' + str(tree), '-I' + str(tree / 'include'),
                           str(fixture), '-L' + str(tree / 'lib'), '-lswoole', '-Wl,-rpath,' + str(tree / 'lib'), '-pthread', '-o', str(consumer)]
                if engine == 'rust':
                    command += ['-DSWOOLE_RS_TIMER=1', '-I' + str(ROOT / 'native/include'), '-I' + str(ROOT / 'native/timer')]
                if args.sanitize:
                    command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
                compiled = run_isolated(command, timeout=90)
                row = {'engine': engine, 'compile': command,
                       'compile_returncode': compiled.returncode, 'compile_stdout': compiled.stdout, 'compile_stderr': compiled.stderr, 'passed': False}
                report['timed_lock_results'].append(row)
                if compiled.returncode:
                    raise RuntimeError(engine + ' timed-lock consumer compile failed')
                observed = run_isolated([str(consumer)], timeout=10, env=environment)
                row.update(returncode=observed.returncode, stdout=observed.stdout, stderr=observed.stderr)
                row['passed'] = observed.returncode == 0 and json.loads(observed.stdout) == {'native_timed_lock_consumers': 3, 'held_timeout': 3, 'unlocked_success': 3}
                write_report(args.output, report)
            if args.ucontext_timers:
                fixture = Path(__file__).with_name('ucontext_timers.cc')
                consumer = tree / 'build/bin/timer-ucontext'
                command = ['c++', '-std=c++17', '-O1', '-g', '-DHAVE_CONFIG_H', '-I' + str(tree), '-I' + str(tree / 'include'),
                           str(fixture), '-L' + str(tree / 'lib'), '-lswoole', '-Wl,-rpath,' + str(tree / 'lib'), '-pthread', '-o', str(consumer)]
                if engine == 'rust':
                    command += ['-DSWOOLE_RS_TIMER=1', '-I' + str(ROOT / 'native/include'), '-I' + str(ROOT / 'native/timer')]
                if args.sanitize:
                    command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
                compiled = run_isolated(command, timeout=90)
                symbols = run_isolated(['nm', '-D', str(tree / 'lib/libswoole.so')], timeout=10)
                imports = [name for name in ['getcontext', 'makecontext', 'swapcontext'] if re.search(r' U ' + name + r'(?:@|$)', symbols.stdout, re.M)]
                row = {'engine': engine, 'compile': command,
                       'compile_returncode': compiled.returncode, 'compile_stdout': compiled.stdout, 'compile_stderr': compiled.stderr,
                       'ucontext_imports': imports, 'passed': False}
                report['ucontext_results'].append(row)
                if compiled.returncode or len(imports) != 3:
                    raise RuntimeError(engine + ' ucontext fixture compile or actual library mode failed')
                observed = run_isolated([str(consumer)], timeout=10, env=environment)
                row.update(returncode=observed.returncode, stdout=observed.stdout, stderr=observed.stderr)
                expected = {'context': 'ucontext', 'expired': 1, 'canceled': 1, 'ticks': 3, 'resumed': 1, 'coroutines': 0, 'timers': 0, 'owner_thread': True}
                row['passed'] = observed.returncode == 0 and json.loads(observed.stdout) == expected
                write_report(args.output, report)
        report['passed'] = len(report['results']) == 18 and all(row['status'] == 'passed' for row in report['results'])
        if args.timed_locks:
            report['passed'] &= len(report['timed_lock_results']) == 2 and all(row['passed'] for row in report['timed_lock_results'])
        if args.ucontext_timers:
            report['passed'] &= len(report['ucontext_results']) == 2 and all(row['passed'] for row in report['ucontext_results'])
    except Exception as error:
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps({'passed': report['passed'], 'results': [(r['engine'], r['name'], r['status']) for r in report['results']], 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
