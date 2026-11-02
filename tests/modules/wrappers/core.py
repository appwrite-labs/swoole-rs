#!/usr/bin/env python3
"""Run the wrapper-related unchanged original M17 GoogleTests in private configured native build trees."""
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
from tools.build_hybrid import overlay_hybrid_tree

from tools.build_upstream import COMMIT as PIN
ASSERTIONS = ['core-tests/src/reactor/base.cpp', 'core-tests/src/os/timer.cpp', 'core-tests/src/server/http_server.cpp']
SELECTED = ['core-tests/src/main.cpp', 'core-tests/src/_lib/http.cpp', *ASSERTIONS]


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
    parser.add_argument('--php-prefix', default='/usr' if sys.platform == 'linux' else subprocess.check_output(['php-config', '--prefix'], text=True).strip(), help='Native PHP prefix for CMake')
    parser.add_argument('--openssl-root', type=Path, help='Match configured native OpenSSL prefix')
    parser.add_argument('--config', type=Path, help='Read-only configured Linux baseline config.h')
    parser.add_argument('--archive', type=Path, help='Rust archive of the hybrid build under test (<build>/rust-prefix/lib/libswoole_rs_ffi.a)')
    parser.add_argument('--build-root', type=Path, help='Private writable cache outside reference/repository')
    parser.add_argument('--library-only', action='store_true', help='Build real native libraries without executing original tests; never original parity')
    parser.add_argument('--inventory-only', action='store_true')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per original test, unchanged stress sizes')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.jobs < 1 or args.timeout < 1:
        parser.error('Jobs/timeout must be positive')
    source = args.source.resolve(strict=True)
    if subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip() != PIN:
        parser.error('Reference pin differs')
    if not args.inventory_only:
        if sys.platform not in {'linux', 'darwin'} or not all([args.config, args.archive, args.build_root]):
            parser.error('Execution requires macOS/Linux and --config, --archive, --build-root')
        for command in ['cmake', 'c++', 'php-config']:
            if not shutil.which(command):
                parser.error('Missing build prerequisite: ' + command)
        build_root = args.build_root.resolve()
        if any(build_root == root or root in build_root.parents for root in [source, ROOT]):
            parser.error('Build root must be outside the reference and repository')
        build_root.mkdir(parents=True, exist_ok=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
              'scope': 'Unchanged original native M17 GoogleTests; isolated full native reference vs wrappers-only overlay. No PHP, combined-module, sanitizer or performance claim.',
              'native_assertions_enabled': True, 'platform': sys.platform, 'library_only': args.library_only,
              'inventory': [], 'engines': [], 'results': [], 'execution_status': 'running', 'passed': False}
    artifacts = args.output.resolve().with_name(args.output.stem + '-artifacts')
    try:
        with tempfile.TemporaryDirectory(prefix='swr-wrappers-core-inputs-') as temporary:
            staged = Path(temporary)
            stage(source, staged)
            inventory = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                for path in ASSERTIONS for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text()) if group != 'http_server' or name in {'parser1', 'parser2', 'upload', 'max_request_size'}]
            if not inventory or len({case['name'] for case in inventory}) != len(inventory):
                raise ValueError('Original wrapper inventory differs')
            report['inventory'] = inventory
            if args.inventory_only:
                report['execution_status'] = 'inventory_only'
            else:
                archive = args.archive.resolve(strict=True)
                # Freeze mutable input once.
                config = args.config.read_bytes()
                for engine in ['upstream', 'rust']:
                    if engine == 'rust':
                        # Every module's patches, as the hybrid build applied them (#796).
                        hybrid_cmake = overlay_hybrid_tree(archive, staged)
                    tree = build_root / ('core-' + engine)
                    for path in staged.rglob('*'):
                        if path.is_file() and '.git' not in path.relative_to(staged).parts:
                            write_changed(tree / path.relative_to(staged), path.read_bytes())
                    write_changed(tree / 'config.h', config)
                    cmake = (staged / 'CMakeLists.txt').read_text()
                    selection = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
                    if cmake.count(selection) != 1:
                        raise ValueError('Original core target selection changed')
                    # Only native library/core-tests are built. The unused PHP
                    # extension target requires unrelated libpq on macOS.
                    if sys.platform == 'darwin':
                        start = cmake.index('file(GLOB ext_cxx_files')
                        end = cmake.index('# core-tests', start)
                        cmake = cmake[:start] + cmake[end:]
                        start = cmake.index('# find libpq')
                        end = cmake.index('# install', start)
                        cmake = cmake[:start] + cmake[end:]
                        cmake = cmake.replace('INSTALL(TARGETS ext-swoole LIBRARY DESTINATION ${PHP_EXTENSION_DIR})', '')
                    if args.openssl_root:
                        cmake = 'link_directories("' + str(args.openssl_root / 'lib') + '")\n' + cmake
                    cmake = cmake.replace(selection, 'set(core_test_files ' + ' '.join(SELECTED) + ')')
                    if engine == 'rust':
                        cmake += f'\n{hybrid_cmake}target_link_libraries(lib-swoole "{archive}")\n'
                    write_changed(tree / 'CMakeLists.txt', cmake.encode())
                    logs = artifacts / engine
                    logs.mkdir(parents=True, exist_ok=True)
                    metadata = {'name': engine}
                    report['engines'].append(metadata)
                    for case in inventory:
                        if sha(tree / case['path']) != case['sha256']:
                            raise ValueError('Original assertion source changed')
                    for name, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=' + args.php_prefix, *(['-DOPENSSL_ROOT_DIR=' + str(args.openssl_root), '-DOPENSSL_INCLUDE_DIR=' + str(args.openssl_root / 'include'), '-DOPENSSL_SSL_LIBRARY=' + str(args.openssl_root / ('lib/libssl.dylib' if sys.platform == 'darwin' else 'lib/libssl.so')), '-DOPENSSL_CRYPTO_LIBRARY=' + str(args.openssl_root / ('lib/libcrypto.dylib' if sys.platform == 'darwin' else 'lib/libcrypto.so'))] if args.openssl_root else []), '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG']),
                            ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'lib-swoole' if args.library_only else 'core-tests', '-j' + str(args.jobs)])]:
                        result = run_isolated(command, timeout=600)
                        log = logs / (name + '.log')
                        log.write_text(result.stdout + result.stderr)
                        metadata[name] = {'command': command, 'returncode': result.returncode, 'log': str(log)}
                        if result.returncode:
                            raise RuntimeError(engine + ' ' + name + ' failed: ' + str(log))
                    if args.library_only:
                        continue
                    binary = tree / 'bin/core-tests'
                    binary.parent.mkdir(exist_ok=True)
                    shutil.copy2(tree / 'build/bin/core-tests', binary)
                    metadata.update(runs=[])
                    for case in inventory:
                        xml = logs / (case['name'] + '.xml')
                        xml.unlink(missing_ok=True)
                        command = [str(binary), '--gtest_filter=' + case['name'], '--gtest_output=xml:' + str(xml)]
                        try:
                            result = run_isolated(command, timeout=args.timeout, cwd=tree, env={**os.environ, 'VERBOSE': '0'})
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
                        report['results'].append({'engine': engine, **case, 'status': status})
                        metadata['runs'].append({'name': case['name'], 'command': command, 'returncode': result.returncode, 'log': str(log), 'xml': str(xml)})
                        write_report(args.output, report)
                report['library_builds_passed'] = args.library_only and len(report['engines']) == 2
                report['unmeasured_original_cases'] = inventory if args.library_only else []
                report['execution_status'] = 'libraries_built' if args.library_only else 'complete'
                report['passed'] = len(report['results']) == 2 * len(inventory) and all(row['status'] == 'passed' for row in report['results'])
    except Exception as error:
        report['execution_status'] = 'failed'
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps({'execution_status': report['execution_status'], 'passed': report['passed'],
                      'results': report['results'], 'error': report.get('error')}))
    return int(not args.inventory_only and not (report['passed'] or report.get('library_builds_passed')) or report['execution_status'] == 'failed')


if __name__ == '__main__':
    sys.exit(main())
