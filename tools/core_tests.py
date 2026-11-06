#!/usr/bin/env python3
"""Run upstream's own GoogleTests (core-tests/src/...) unchanged on the pinned reference and on the
hybrid build's patched tree (overlay_hybrid_tree), in private Linux build trees.

Every TEST() in each --file is a case. A case passes when it passes on both engines. --exclude
NAME=REASON names a case that cannot run here (network, IPv6, an external service); it is reported
`excluded` with its reason and never counts as passed."""
import argparse
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

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_hybrid import overlay_hybrid_tree  # noqa: E402
from tools.build_upstream import COMMIT as PIN  # noqa: E402
from tools.harness import run_isolated, write_report  # noqa: E402

SELECTION = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
# Upstream's test helpers (not assertions) that write state the hybrid moved behind accessors.
ADAPTATIONS = {
    'core-tests/src/_lib/process.cpp': [('worker.pid = pid;', 'worker.set_pid(pid);')],
    # Failure cleanup still shuts down the actual listener after a failed sender.
    'core-tests/src/os/process_pool.cpp': [
        ('pool.stream_info_->socket->shutdown(SHUT_RDWR);',
         'static_cast<swoole::network::Socket *>(swr_v1_process_pool_ipc_listener(pool.ipc_))->shutdown(SHUT_RDWR);')],
}
# Headers the hybrid folded into another one; the hybrid's core-tests include the survivor.
MOVED_HEADERS = {'swoole_process_pool.h': 'swoole_server.h'}
# The original ftruncate case creates this file with mode 000 and leaves it behind.
# Give each engine a private path; keep the open flags, mode and every assertion.
ABSOLUTE_FIXTURES = {'coroutine_hook.ftruncate': ('core-tests/src/coroutine/hook.cpp', '/tmp/123.txt')}


def private_fixture(tree, name):
    return tree / 'case-fixtures' / name / Path(ABSOLUTE_FIXTURES[name][1]).name


def write_changed(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)


def stage(source, destination):
    """The pin's tree, including core-tests, which `git archive` leaves out (export-ignore)."""
    with tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git', '-C', str(source), 'archive', PIN]))) as bundle:
        for member in bundle:
            if member.isfile():
                write_changed(destination / member.name, bundle.extractfile(member).read())
    names = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', PIN, 'core-tests', 'examples/test.jpg'],
                                    text=True).splitlines()
    blobs = subprocess.check_output(['git', '-C', str(source), 'cat-file', '--batch'], input=''.join(f'{PIN}:{name}\n' for name in names).encode())
    offset = 0
    for name in names:
        end = blobs.index(b'\n', offset)
        size = int(blobs[offset:end].split()[-1])
        write_changed(destination / name, blobs[end + 1:end + 1 + size])
        offset = end + 2 + size


def cases(staged, files):
    return [f'{suite}.{name}' for path in files for suite, name in re.findall(r'^TEST\(\s*(\w+)\s*,\s*(\w+)\s*\)', (staged / path).read_text(), re.M)]


def run_case(binary, tree, name, logs, timeout):
    if name in ABSOLUTE_FIXTURES:
        fixture = private_fixture(tree, name)
        fixture.parent.mkdir(parents=True, exist_ok=True)
        fixture.unlink(missing_ok=True)
    xml = logs / (name + '.xml')
    xml.unlink(missing_ok=True)
    command = [str(binary), '--gtest_filter=' + name, '--gtest_output=xml:' + str(xml)]
    try:
        result = run_isolated(command, timeout=timeout, cwd=tree, env={**os.environ, 'VERBOSE': '0'})
    except subprocess.TimeoutExpired as error:
        result = subprocess.CompletedProcess(command, 124, '', f'Timed out: {error}')
    (logs / (name + '.log')).write_text(result.stdout + result.stderr)
    rows = list(ET.parse(xml).iter('testcase')) if xml.is_file() else []
    if result.returncode or len(rows) != 1 or rows[0].get('classname') + '.' + rows[0].get('name') != name:
        return 'failed'
    row = rows[0]
    if row.find('failure') is not None or row.find('error') is not None:
        return 'failed'
    return 'skipped' if row.find('skipped') is not None or row.get('status') != 'run' else 'passed'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True, help='Pinned upstream checkout')
    parser.add_argument('--config', type=Path, required=True, help='The hybrid build\'s configured config.h')
    parser.add_argument('--archive', type=Path, required=True, help='<build>/rust-prefix/lib/libswoole_rs_ffi.a of the hybrid build')
    parser.add_argument('--build-root', type=Path, required=True, help='Private writable build directory')
    parser.add_argument('--file', action='append', required=True, help='core-tests/src/... file whose TEST()s run')
    parser.add_argument('--exclude', action='append', default=[], metavar='SUITE.NAME=REASON')
    parser.add_argument('--fixture', action='append', default=[], metavar='FILE=SUITE.NAME',
                        help='An upstream test that must pass first because the cases read what it writes')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per test')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('core-tests build on Linux only')
    excluded = dict(entry.split('=', 1) for entry in args.exclude)
    fixtures = dict(entry.split('=', 1) for entry in args.fixture)
    report = {'pin': PIN, 'scope': __doc__, 'files': args.file, 'excluded': excluded, 'engines': [], 'results': [], 'passed': False}
    artifacts = args.output.resolve().with_name(args.output.stem + '-artifacts')
    try:
        with tempfile.TemporaryDirectory(prefix='swr-core-tests-') as temporary:
            staged = Path(temporary)
            stage(args.source, staged)
            inventory = cases(staged, args.file)
            if not inventory or len(set(inventory)) != len(inventory) or set(excluded) - set(inventory):
                raise ValueError('Empty or duplicate inventory, or an exclusion names no case')
            for engine in ['upstream', 'hybrid']:
                hybrid_cmake = overlay_hybrid_tree(args.archive, staged) if engine == 'hybrid' else ''
                tree = args.build_root.resolve() / engine
                for path in staged.rglob('*'):
                    if path.is_file():
                        write_changed(tree / path.relative_to(staged), path.read_bytes())
                write_changed(tree / 'config.h', args.config.read_bytes())
                for path, edits in (ADAPTATIONS.items() if engine == 'hybrid' else []):
                    if path not in args.file and '/_lib/' not in path:
                        continue
                    text = (staged / path).read_text()
                    for old, new in edits:
                        if text.count(old) != 1:
                            raise ValueError(f'Test helper changed: {path}')
                        text = text.replace(old, new)
                    write_changed(tree / path, text.encode())
                for path in (tree / 'core-tests').rglob('*') if engine == 'hybrid' else []:
                    if path.suffix not in ('.h', '.cpp'):
                        continue
                    text = path.read_text()
                    for old, new in MOVED_HEADERS.items():
                        text = text.replace(f'#include "{old}"', f'#include "{new}"')
                    write_changed(path, text.encode())
                for name, (path, original) in ABSOLUTE_FIXTURES.items():
                    if path not in args.file:
                        continue
                    text = (tree / path).read_text()
                    if text.count(json.dumps(original)) != 1:
                        raise ValueError(f'Absolute test fixture changed: {path}')
                    replacement = private_fixture(tree, name)
                    text = text.replace(json.dumps(original), json.dumps(str(replacement)))
                    write_changed(tree / path, text.encode())
                    report.setdefault('fixture_isolation', []).append(
                        {'engine': engine, 'case': name, 'original': original, 'private': str(replacement)})
                cmake = (staged / 'CMakeLists.txt').read_text()
                if cmake.count(SELECTION) != 1:
                    raise ValueError('Upstream core-tests target selection changed')
                cmake = cmake.replace(SELECTION, 'file(GLOB core_test_files ' + ' '.join(['core-tests/src/main.cpp', 'core-tests/src/_lib/*.cpp', *fixtures, *args.file]) + ')')
                if hybrid_cmake:
                    cmake += f'\n{hybrid_cmake}target_link_libraries(lib-swoole "{args.archive.resolve()}")\n'
                write_changed(tree / 'CMakeLists.txt', cmake.encode())
                logs = artifacts / engine
                logs.mkdir(parents=True, exist_ok=True)
                for step, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release',
                                                     '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG']),
                                      ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'core-tests', '-j' + str(args.jobs)])]:
                    result = run_isolated(command, timeout=900)
                    (logs / (step + '.log')).write_text(result.stdout + result.stderr)
                    if result.returncode:
                        raise RuntimeError(f'{engine} {step} failed: {logs / (step + ".log")}')
                report['engines'].append(engine)
                binary = tree / 'bin/core-tests'
                binary.parent.mkdir(exist_ok=True)
                shutil.copy2(tree / 'build/bin/core-tests', binary)
                for name in fixtures.values():
                    if run_case(binary, tree, name, logs, args.timeout) != 'passed':
                        raise RuntimeError(f'{engine} fixture {name} failed: {logs / (name + ".log")}')
                for name in inventory:
                    status = 'excluded' if name in excluded else run_case(binary, tree, name, logs, args.timeout)
                    report['results'].append({'engine': engine, 'name': name, 'status': status})
                    write_report(args.output, report)
        report['passed'] = all(row['status'] in ('passed', 'excluded') for row in report['results'])
    except Exception as error:
        report['error'] = str(error)
    write_report(args.output, report)
    failed = [f"{row['engine']}:{row['name']}" for row in report['results'] if row['status'] not in ('passed', 'excluded')]
    print(json.dumps({'passed': report['passed'], 'cases': len(report['results']), 'failed': failed, 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
