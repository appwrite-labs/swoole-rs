#!/usr/bin/env python3
"""Run the unchanged original M06 coroutine GoogleTests with actual native contexts."""
import argparse
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.build_upstream import require_build_space
from tools.build_hybrid import overlay_hybrid_tree

from tools.build_upstream import COMMIT as PIN
ASSERTIONS = ['core-tests/src/coroutine/base.cpp', 'core-tests/src/coroutine/channel.cpp']
LIFETIME = 'core-tests/src/coroutine/rust_policy_lifetime.cpp'
SELECTED = ['core-tests/src/main.cpp', *ASSERTIONS, LIFETIME]
# Run without LeakSanitizer (#415), by name only; their assertions are unchanged and any
# other failure still fails the phase. Every other case keeps leak checking.
SUSPENDED = 'the original leaves a coroutine suspended forever; its stack holds the only reference'
NO_LEAK_CHECK = {'coroutine_base.bailout': SUSPENDED, 'coroutine_channel.push_yield': SUSPENDED,
                 'coroutine_channel.close': SUSPENDED}


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_changed(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)


def stage(source, destination):
    # core-tests is export-ignored. Recover its immutable blobs, not local files.
    packed = subprocess.check_output(['git', '-C', str(source), 'archive', PIN])
    with tarfile.open(fileobj=io.BytesIO(packed)) as bundle:
        for member in bundle:
            path = Path(member.name)
            if path.is_absolute() or '..' in path.parts:
                raise ValueError('Unsafe archive member')
            if member.isfile():
                write_changed(destination / path, bundle.extractfile(member).read())
    names = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', PIN, 'core-tests', 'examples/test.jpg'], text=True).splitlines()
    blobs = subprocess.check_output(['git', '-C', str(source), 'cat-file', '--batch'],
                                   input=''.join(f'{PIN}:{name}\n' for name in names).encode())
    offset = 0
    for name in names:
        end = blobs.index(b'\n', offset)
        size = int(blobs[offset:end].split()[-1])
        offset = end + 1
        write_changed(destination / name, blobs[offset:offset + size])
        offset += size + 1
    subprocess.run(['git', 'init', '-q', str(destination)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--config', type=Path, help='Read-only same-platform configured baseline config.h')
    parser.add_argument('--archive', type=Path, help='Same-platform real coroutine Rust static archive')
    parser.add_argument('--build-root', type=Path, help='Private writable cache outside reference/repository')
    parser.add_argument('--dns-requests', action='store_true', help='Execute the unchanged local DNS no-service original and five real request/async/vector witnesses')
    parser.add_argument('--socket-scopes', action='store_true', help='Also execute the two unchanged original local socket timeout tests and three scoped-state witnesses')
    parser.add_argument('--async-file', action='store_true', help='Also compile and execute the unchanged original coroutine AsyncFile unit')
    parser.add_argument('--lifetime-only', action='store_true', help='Execute only three supplemental real-context lifetime witnesses; original32 remain unmeasured')
    parser.add_argument('--rust-asan', action='store_true', help='Verify the coroutine archive contains actual Rust ASan imports')
    parser.add_argument('--ucontext', action='store_true', help='Derived private config explicitly selects native ucontext instead of assembly')
    parser.add_argument('--sanitize', action='store_true', help='Native full-core ASan; Rust archive instrumentation remains separate')
    parser.add_argument('--gtest-prefix', type=Path, help='Read-only C++14-compatible GoogleTest installation')
    parser.add_argument('--php-prefix', default='/usr', help='Configured php-config prefix')
    parser.add_argument('--inventory-only', action='store_true')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per original test, unchanged stress sizes')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if (args.async_file or args.socket_scopes or args.dns_requests) and args.lifetime_only:
        parser.error('--async-file requires the complete original selection')
    selected_units = [*SELECTED, *(['core-tests/src/coroutine/file.cpp'] if args.async_file else [])]
    if args.socket_scopes:
        selected_units += ['core-tests/src/coroutine/socket.cpp', 'core-tests/src/_lib/server.cpp',
                           'core-tests/src/_lib/process.cpp', 'core-tests/src/coroutine/rust_socket_scopes.cpp']
    if args.dns_requests:
        selected_units += ['core-tests/src/network/dns.cpp', 'core-tests/src/coroutine/rust_dns_requests.cpp']
    if args.socket_scopes or args.dns_requests:
        selected_units += ['core-tests/src/coroutine/rust_fault_allocation.cpp']
    if args.jobs < 1 or args.timeout < 1:
        parser.error('Jobs/timeout must be positive')
    source = args.source.resolve(strict=True)
    if subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip() != PIN:
        parser.error('Reference pin differs')
    if not args.inventory_only:
        if not all([args.config, args.archive, args.build_root]):
            parser.error('Execution requires --config, --archive, --build-root')
        for command in ['cmake', 'c++', 'php-config']:
            if not shutil.which(command):
                parser.error('Missing build prerequisite: ' + command)
        build_root = args.build_root.resolve()
        if any(build_root == root or root in build_root.parents for root in [source]):
            parser.error('Build root must be outside the reference and repository')
        build_root.mkdir(parents=True, exist_ok=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
              'scope': 'Unchanged original native M06 GoogleTests; isolated full native reference vs coroutine-only overlay. Actual original context switching; no PHP, combined-module or performance claim. Sanitizer scope is explicit.',
              'native_assertions_enabled': True, 'rust_asan_requested': args.rust_asan, 'native_asan': args.sanitize, 'gtest_prefix': str(args.gtest_prefix) if args.gtest_prefix else None,
              'inventory': [], 'engines': [], 'results': [], 'execution_status': 'running', 'passed': False}
    artifacts = args.output.resolve().with_name(args.output.stem + '-artifacts')
    try:
        with tempfile.TemporaryDirectory(prefix='swr-coroutine-core-inputs-',
                                         dir=None if args.inventory_only else build_root) as temporary:
            staged = Path(temporary)
            require_build_space(staged)
            stage(source, staged)
            lifetime = (ROOT / 'tests/modules/coroutine/lifetime.cpp').read_bytes()
            write_changed(staged / LIFETIME, lifetime)
            inventory = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                for path in ASSERTIONS for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
            if len(inventory) != 33 or len({case['name'] for case in inventory}) != len(inventory):
                raise ValueError('Original coroutine inventory differs')
            if args.async_file:
                path = 'core-tests/src/coroutine/file.cpp'
                extra = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                         for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
                if [case['name'] for case in extra] != ['coroutine_async_file.async_file']:
                    raise ValueError('Original AsyncFile inventory differs')
                inventory += extra
                report['scope'] = 'Unchanged33 coroutine originals plus AsyncFile and3 supplemental lifetime witnesses; full native reference vs the exact production archive overlay. No PHP or performance claim.'
            if args.socket_scopes:
                path = 'core-tests/src/coroutine/socket.cpp'
                all_socket = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                              for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
                names = {'coroutine_socket.timeout_setter', 'coroutine_socket.timeout_controller'}
                selected_socket = [case for case in all_socket if case['name'] in names]
                if {case['name'] for case in selected_socket} != names:
                    raise ValueError('Original socket timeout inventory differs')
                inventory += selected_socket
                report['unmeasured_socket_originals'] = [case['name'] for case in all_socket if case['name'] not in names]
                report['scope'] = 'Unchanged coroutine originals and two local socket timeout originals, plus lifetime and scoped-state witnesses; actual complete production overlay. Other socket originals remain unmeasured.'
            if args.dns_requests:
                path = 'core-tests/src/network/dns.cpp'
                all_dns = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                           for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
                selected_dns = [case for case in all_dns if case['name'] == 'dns.getaddrinfo_without_service']
                if len(selected_dns) != 1:
                    raise ValueError('Original DNS no-service inventory differs')
                inventory += selected_dns
                report['unmeasured_dns_originals'] = [case['name'] for case in all_dns if case not in selected_dns]
                report['scope'] = 'Unchanged coroutine originals and local DNS no-service original, plus selected socket originals and lifetime/owner witnesses; actual production overlay. Other DNS/socket originals explicitly unmeasured.'
            report['original_inventory'] = inventory.copy()
            report['unmeasured_originals'] = [case['name'] for case in inventory] if args.lifetime_only else []
            supplemental = [{'name': group + '.' + name, 'path': LIFETIME, 'sha256': hashlib.sha256(lifetime).hexdigest()} for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', lifetime.decode())]
            if len(supplemental) != 3:
                raise ValueError('Supplemental lifetime inventory differs')
            if args.socket_scopes:
                path = 'core-tests/src/coroutine/rust_socket_scopes.cpp'
                data = (ROOT / 'tests/modules/hooks/socket_scopes.cpp').read_bytes()
                write_changed(staged / path, data)
                supplemental += [{'name': group + '.' + name, 'path': path, 'sha256': hashlib.sha256(data).hexdigest()}
                                 for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', data.decode())]
                if len(supplemental) != 6:
                    raise ValueError('Socket scope witness inventory differs')
            if args.dns_requests:
                path = 'core-tests/src/coroutine/rust_dns_requests.cpp'
                data = (ROOT / 'tests/modules/hooks/dns_request_owner.cpp').read_bytes()
                write_changed(staged / path, data)
                extra = [{'name': group + '.' + name, 'path': path, 'sha256': hashlib.sha256(data).hexdigest()}
                         for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', data.decode())]
                if len(extra) != 5:
                    raise ValueError('DNS owner witness inventory differs')
                supplemental += extra
            if args.socket_scopes or args.dns_requests:
                write_changed(staged / 'core-tests/src/coroutine/swoole_rs_test_allocation.h',
                              b'#include <cstddef>\nnamespace allocation { void begin(size_t); void end(); }\n')
                write_changed(staged / 'core-tests/src/coroutine/swoole_rs_allocation_definitions.h',
                              (ROOT / 'tests/modules/http/allocation_oracle.h').read_bytes())
                write_changed(staged / 'core-tests/src/coroutine/rust_fault_allocation.cpp',
                              (ROOT / 'tests/modules/hooks/fault_allocation.cpp').read_bytes())
            inventory = supplemental if args.lifetime_only else inventory + supplemental
            report['inventory'] = inventory
            report['supplemental_inventory'] = supplemental
            if args.inventory_only:
                report['execution_status'] = 'inventory_only'
            else:
                archive = args.archive.resolve(strict=True)
                archive_data = archive.read_bytes()
                # Freeze mutable input once.
                if args.rust_asan:
                    symbols = subprocess.run(['nm', '-u', str(archive)], capture_output=True, text=True, check=True).stdout
                    if '__asan_report_' not in symbols:
                        raise ValueError('Rust ASan requested but archive has no ASan report imports')
                    report['rust_standard_library_instrumented'] = False
                config = args.config.read_bytes()
                if re.search(rb'^#define SW_USE_THREAD_CONTEXT\b', config, re.M):
                    raise ValueError('Optional thread-context requires separate ownership/lifetime proof')
                if args.ucontext:
                    config = re.sub(rb'^#define SW_USE_ASM_CONTEXT\b[^\n]*', b'#undef SW_USE_ASM_CONTEXT', config, flags=re.M)
                report['native_context_mode'] = 'asm' if re.search(rb'^#define SW_USE_ASM_CONTEXT\b', config, re.M) else 'ucontext'
                for engine in ['upstream', 'rust']:
                    if engine == 'rust':
                        # Every module's patches, as the hybrid build applied them (#796).
                        hybrid_cmake = overlay_hybrid_tree(archive, staged)
                    tree = build_root / ('core-' + engine)
                    for path in staged.rglob('*'):
                        if path.is_file() and '.git' not in path.relative_to(staged).parts:
                            write_changed(tree / path.relative_to(staged), path.read_bytes())
                    write_changed(tree / 'config.h', config)
                    frozen_archive = tree / '.frozen-archive/libswoole_rs_coroutine.a'
                    write_changed(frozen_archive, archive_data)
                    cmake = (staged / 'CMakeLists.txt').read_text()
                    selection = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
                    if cmake.count(selection) != 1:
                        raise ValueError('Original core target selection changed')
                    cmake = cmake.replace(selection, 'set(core_test_files ' + ' '.join(selected_units) + ')')
                    if engine == 'rust':
                        cmake += f'\n{hybrid_cmake}target_link_libraries(lib-swoole "{frozen_archive}")\n'
                    write_changed(tree / 'CMakeLists.txt', cmake.encode())
                    logs = artifacts / engine
                    logs.mkdir(parents=True, exist_ok=True)
                    metadata = {'name': engine}
                    report['engines'].append(metadata)
                    for case in inventory:
                        if sha(tree / case['path']) != case['sha256']:
                            raise ValueError('Original assertion source changed')
                    for name, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir='+args.php_prefix, '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG', *(['-DGTest_DIR='+str(args.gtest_prefix.resolve() / 'lib/cmake/GTest')] if args.gtest_prefix else []), *(['-Dlibpq_dir=/opt/homebrew/opt/libpq', '-Dopenssl_dir=/opt/homebrew/opt/openssl@3', '-DCMAKE_EXE_LINKER_FLAGS=-L/opt/homebrew/lib'] if sys.platform == 'darwin' else []), *(['-Denable_asan=1'] if args.sanitize else ['-Uenable_asan'])]),
                            ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'core-tests', '-j' + str(args.jobs)])]:
                        require_build_space(tree)
                        result = run_isolated(command, timeout=600)
                        log = logs / (name + '.log')
                        log.write_text(result.stdout + result.stderr)
                        metadata[name] = {'command': command, 'returncode': result.returncode, 'log': str(log)}
                        if result.returncode:
                            raise RuntimeError(engine + ' ' + name + ' failed: ' + str(log))
                    binary = tree / 'bin/core-tests'
                    binary.parent.mkdir(exist_ok=True)
                    shutil.copy2(tree / 'build/bin/core-tests', binary)
                    metadata.update(compiler_flags={name: (tree / ('build/CMakeFiles/'+name+'.dir/flags.make')).read_text() for name in ['lib-swoole', 'core-tests']}, runs=[])
                    library = tree / ('lib/libswoole.dylib' if sys.platform == 'darwin' else 'lib/libswoole.so')
                    # tls-native takes the upstream engine as its attested native control (#416).
                    metadata.update(library_sha256=sha(library), compiled_config_sha256=sha(tree / 'config.h'))
                    symbols = subprocess.check_output(['nm', '-u', str(library)], text=True)
                    metadata['ucontext_imports'] = {name: name in symbols for name in ['getcontext', 'makecontext', 'swapcontext']}
                    if report['native_context_mode'] == 'ucontext' and not all(metadata['ucontext_imports'].values()):
                        raise RuntimeError('ucontext selection lacks actual library imports')

                    for case in inventory:
                        xml = logs / (case['name'] + '.xml')
                        xml.unlink(missing_ok=True)
                        command = [str(binary), '--gtest_filter=' + case['name'], '--gtest_output=xml:' + str(xml)]
                        env = {**os.environ, 'VERBOSE': '0'}
                        if case['name'] in NO_LEAK_CHECK:
                            env['ASAN_OPTIONS'] = ':'.join(filter(None, [env.get('ASAN_OPTIONS'), 'detect_leaks=0']))
                        try:
                            result = run_isolated(command, timeout=args.timeout, cwd=tree, env=env)
                        except subprocess.TimeoutExpired as error:
                            result = subprocess.CompletedProcess(command, 124, '', 'Timed out: ' + str(error))
                        log = logs / (case['name'] + '.log')
                        log.write_text(result.stdout + result.stderr)
                        rows = [] if not xml.is_file() else list(ET.parse(xml).iter('testcase'))
                        valid = len(rows) == 1 and rows[0].attrib.get('classname') + '.' + rows[0].attrib.get('name') == case['name']
                        status = 'failed'
                        if valid and not result.returncode:
                            row = rows[0]
                            status = 'failed' if row.find('failure') is not None or row.find('error') is not None else 'skipped' if row.find('skipped') is not None or row.attrib.get('status') != 'run' else 'passed'
                        warnings = [line for line in (result.stdout + result.stderr).splitlines() if 'WARNING: ASan' in line or 'WARNING: AddressSanitizer' in line]
                        # Only the hybrid's context switches are annotated for ASan; the unmodified
                        # reference's warnings are upstream's behaviour, recorded but not judged (#415).
                        if status == 'passed' and warnings and engine == 'rust':
                            status = 'warned'
                        report['results'].append({'engine': engine, **case, 'status': status, 'sanitizer_warnings': warnings,
                                                  'leak_check': case['name'] not in NO_LEAK_CHECK,
                                                  **({'no_leak_check_reason': NO_LEAK_CHECK[case['name']]} if case['name'] in NO_LEAK_CHECK else {})})
                        metadata['runs'].append({'name': case['name'], 'command': command, 'returncode': result.returncode, 'log': str(log), 'xml': str(xml)})
                        write_report(args.output, report)
                report['execution_status'] = 'complete'
                report['selected_passed'] = bool(report['results']) and all(row['status'] == 'passed' for row in report['results'])
                report['passed'] = len(report['results']) == 2 * len(inventory) and not report['unmeasured_originals'] and all(row['status'] == 'passed' for row in report['results'])
    except Exception as error:
        report['execution_status'] = 'failed'
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps({'execution_status': report['execution_status'], 'passed': report['passed'],
                      'results': report['results'], 'error': report.get('error')}))
    return int(not args.inventory_only and not report['passed'] or report['execution_status'] == 'failed')


if __name__ == '__main__':
    sys.exit(main())
