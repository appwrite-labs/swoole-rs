#!/usr/bin/env python3
"""Run the 20 unchanged original M12 GoogleTests in private Linux build trees."""
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
ASSERTIONS = ['core-tests/src/memory/' + name + '.cpp' for name in
              ['fixed_pool', 'global_memory', 'ringbuffer', 'table']]
ASSERTIONS += ['core-tests/src/core/' + name + '.cpp' for name in ['heap', 'channel']]


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
    names = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', PIN, 'core-tests'], text=True).splitlines()
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
    parser.add_argument('--config', type=Path, help='Read-only configured Linux baseline config.h')
    parser.add_argument('--archive', type=Path, help='Rust archive of the hybrid build under test (<build>/rust-prefix/lib/libswoole_rs_ffi.a)')
    parser.add_argument('--build-root', type=Path, help='Private writable cache outside reference/repository')
    parser.add_argument('--inventory-only', action='store_true')
    parser.add_argument('--model', action='store_true', help='Also run owned standalone FIFO/table/fork models, bounded mapping and native allocation-failure witnesses')
    parser.add_argument('--sanitize', action='store_true', help='Native AddressSanitizer only; original packed C++ is not UBSan-clean')
    parser.add_argument('--rust-asan', action='store_true', help='Supplied archive was separately compiled with actual Rust ASan')
    parser.add_argument('--case', action='append', default=[], help='Exact original group.name; repeat to select a focused correctness loop')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per original test, unchanged stress sizes')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.jobs < 1 or args.timeout < 1:
        parser.error('Jobs/timeout must be positive')
    if args.rust_asan and (not args.sanitize or args.inventory_only):
        parser.error('--rust-asan requires --sanitize and actual execution')
    source = args.source.resolve(strict=True)
    if subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip() != PIN:
        parser.error('Reference pin differs')
    if not args.inventory_only:
        if sys.platform != 'linux' or not all([args.config, args.archive, args.build_root]):
            parser.error('Execution requires Linux and --config, --archive, --build-root')
        for command in ['cmake', 'c++', 'php-config']:
            if not shutil.which(command):
                parser.error('Missing build prerequisite: ' + command)
        build_root = args.build_root.resolve()
        if any(build_root == root or root in build_root.parents for root in [source, ROOT]):
            parser.error('Build root must be outside the reference and repository')
        build_root.mkdir(parents=True, exist_ok=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
              'scope': 'Unchanged original native M12 GoogleTests; isolated full native reference vs memory-only overlay. No PHP, combined-module, sanitizer or performance claim.',
              'native_assertions_enabled': True, 'native_asan': args.sanitize, 'rust_asan_declared': args.rust_asan,
              'inventory': [], 'engines': [], 'results': [], 'models': [], 'execution_status': 'running', 'passed': False, 'whole_inventory_passed': False}
    environment = dict(os.environ, VERBOSE='0')
    if args.sanitize:
        environment['ASAN_OPTIONS'] = 'detect_leaks=1:abort_on_error=1'
        report['sanitizer_options'] = {'ASAN_OPTIONS': environment['ASAN_OPTIONS']}
        report['scope'] = 'Unchanged original native M12 GoogleTests with native AddressSanitizer. Rust instrumentation is separately declared/checked; precompiled std is uninstrumented. No UBSan, PHP, combined-module or performance claim.'
    artifacts = args.output.resolve().with_name(args.output.stem + '-artifacts')
    try:
        with tempfile.TemporaryDirectory(prefix='swr-memory-core-inputs-') as temporary:
            staged = Path(temporary)
            stage(source, staged)
            inventory = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                for path in ASSERTIONS for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
            if len(inventory) != 20 or len({case['name'] for case in inventory}) != 20:
                raise ValueError('Original 20-case inventory differs')
            report['inventory'] = inventory
            unknown = set(args.case) - {case['name'] for case in inventory}
            if unknown:
                raise ValueError('Unknown original case: ' + ', '.join(sorted(unknown)))
            selected = [case for case in inventory if not args.case or case['name'] in args.case]
            report['selected_cases'] = [case['name'] for case in selected]
            report['unmeasured_cases'] = [case['name'] for case in inventory if case not in selected]
            if args.inventory_only:
                report['execution_status'] = 'inventory_only'
            else:
                archive = args.archive.resolve(strict=True)
                if args.rust_asan:
                    symbols = subprocess.check_output(['nm', '-u', str(archive)], stderr=subprocess.STDOUT, text=True)
                    imports = sorted(set(re.findall(r'__asan_report_\w+', symbols)))
                    if not imports:
                        raise ValueError('Declared Rust ASan archive lacks address-report instrumentation imports')
                    report['rust_asan_imports'] = imports
                # Freeze mutable input once.
                config = args.config.read_bytes()
                for engine in ['upstream', 'rust']:
                    if engine == 'rust':
                        # Every module's patches, as the hybrid build applied them (#796).
                        flags = overlay_hybrid_tree(archive, staged)
                    tree = build_root / ('core-' + engine)
                    for path in staged.rglob('*'):
                        if path.is_file() and '.git' not in path.relative_to(staged).parts:
                            write_changed(tree / path.relative_to(staged), path.read_bytes())
                    write_changed(tree / 'config.h', config)
                    cmake = (staged / 'CMakeLists.txt').read_text()
                    selection = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
                    if cmake.count(selection) != 1:
                        raise ValueError('Original core target selection changed')
                    cmake = cmake.replace(selection, 'set(core_test_files ' + ' '.join(['core-tests/src/main.cpp', *dict.fromkeys(case['path'] for case in selected)]) + ')')
                    if engine == 'rust':
                        cmake += f'\nadd_compile_options({" ".join(flags)})\ntarget_link_libraries(lib-swoole "{archive}")\n'
                    write_changed(tree / 'CMakeLists.txt', cmake.encode())
                    logs = artifacts / engine
                    logs.mkdir(parents=True, exist_ok=True)
                    metadata = {'name': engine}
                    report['engines'].append(metadata)
                    for case in inventory:
                        if sha(tree / case['path']) != case['sha256']:
                            raise ValueError('Original assertion source changed')
                    flags = '-fsanitize=address -fno-omit-frame-pointer' if args.sanitize else ''
                    configure = ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_CXX_FLAGS=' + flags, '-DCMAKE_C_FLAGS=' + flags, '-DCMAKE_EXE_LINKER_FLAGS=' + flags, '-DCMAKE_SHARED_LINKER_FLAGS=' + flags]
                    for name, command in [('configure', configure),
                            ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'core-tests', '-j' + str(args.jobs)])]:
                        result = run_isolated(command, timeout=600)
                        log = logs / (name + '.log')
                        log.write_text(result.stdout + result.stderr)
                        metadata[name] = {'command': command, 'returncode': result.returncode, 'log': str(log)}
                        if result.returncode:
                            raise RuntimeError(engine + ' ' + name + ' failed: ' + str(log))
                    binary = tree / 'bin/core-tests'
                    binary.parent.mkdir(exist_ok=True)
                    shutil.copy2(tree / 'build/bin/core-tests', binary)
                    metadata['runs'] = []
                    for case in selected:
                        xml = logs / (case['name'] + '.xml')
                        xml.unlink(missing_ok=True)
                        command = [str(binary), '--gtest_filter=' + case['name'], '--gtest_output=xml:' + str(xml)]
                        try:
                            result = run_isolated(command, timeout=args.timeout, cwd=tree, env=environment)
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
                    if args.model:
                        binary = tree / 'bin/memory-model'
                        fixtures = [ROOT / 'tests/modules/memory' / name for name in ['probe.cc', 'standalone.cc']]
                        command = ['c++', '-std=c++17', '-O2', '-UNDEBUG', '-DHAVE_CONFIG_H',
                                   '-I' + str(tree / 'include'), '-I' + str(tree),
                                   '-I' + str(ROOT / 'native/memory'), *map(str, fixtures),
                                   '-L' + str(tree / 'lib'), '-Wl,-rpath,' + str(tree / 'lib'),
                                   '-lswoole', '-pthread', '-o', str(binary)]
                        if args.sanitize:
                            command += ['-fsanitize=address', '-fno-omit-frame-pointer']
                        if engine == 'rust':
                            command += ['-DSWOOLE_RS_MEMORY']
                        result = run_isolated(command, timeout=60)
                        (logs / 'model-build.log').write_text(result.stdout + result.stderr)
                        if result.returncode:
                            raise RuntimeError('Standalone model build failed: ' + str(logs / 'model-build.log'))
                        metadata['model'] = {'command': command}
                        for mode in ['', 'mapping', 'metadata', 'odd-rows', 'creation', 'clearing'] if engine == 'rust' else ['']:
                            command = [str(binary)] + ([mode] if mode else [])
                            result = run_isolated(command, timeout=args.timeout, cwd=tree, env=environment)
                            log = logs / ('model-' + (mode or 'collections') + '.log')
                            log.write_text(result.stdout + result.stderr)
                            observations = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                            valid = len(observations) == 1 and observations[0].get('passed') is True
                            report['models'].append({'engine': engine, 'mode': mode or 'collections',
                                'passed': result.returncode == 0 and valid, 'returncode': result.returncode,
                                'log': str(log), 'observations': observations})
                            write_report(args.output, report)
                report['execution_status'] = 'complete'
                report['passed'] = bool(selected) and len(report['results']) == 2 * len(selected) and all(row['status'] == 'passed' for row in report['results'])
                if args.model:
                    paired = [row['observations'] for row in report['models'] if row['mode'] == 'collections']
                    report['model_observations_match'] = len(paired) == 2 and paired[0] == paired[1]
                    report['passed'] = report['passed'] and all(row['passed'] for row in report['models']) and report['model_observations_match']
                report['whole_inventory_passed'] = report['passed'] and len(selected) == len(inventory)
    except Exception as error:
        report['execution_status'] = 'failed'
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps({'execution_status': report['execution_status'], 'passed': report['passed'],
                      'results': report['results'], 'error': report.get('error')}))
    return int(not args.inventory_only and not report['passed'] or report['execution_status'] == 'failed')


if __name__ == '__main__':
    sys.exit(main())
