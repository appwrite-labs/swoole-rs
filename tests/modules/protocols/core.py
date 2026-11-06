#!/usr/bin/env python3
"""Build/run unchanged original M13 GTest assertions in private Linux trees."""
import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, stop_process_group, write_report
from tools.build_hybrid import overlay_hybrid_tree

from tools.build_upstream import COMMIT as PIN
ASSERTIONS = ['core-tests/src/protocol/redis.cpp', 'core-tests/src/server/message_bus.cpp', 'core-tests/src/server/mqtt.cpp']
SELECTED = ['core-tests/src/main.cpp', 'core-tests/src/_lib/redis.cpp', *ASSERTIONS]
# The hybrid builds no redis::format/parse (nothing but redis.cpp calls them), so its
# assertions reach Rust's RESP codec through the protocols gates' adapter.
REDIS_REPLY = 'core-tests/src/_lib/redis_reply.cpp'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def write_changed(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)


def staged_sources(source, destination):
    """git archive excludes core-tests; recover those exact blobs explicitly."""
    packed = subprocess.check_output(['git', '-C', str(source), 'archive', PIN])
    with tarfile.open(fileobj=io.BytesIO(packed)) as bundle:
        for member in bundle:
            if not member.isfile():
                continue
            path = Path(member.name)
            if path.is_absolute() or '..' in path.parts:
                raise ValueError('Unsafe archive member')
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


@contextmanager
def redis_fixture(directory):
    """Never reuse an existing service: only test against our owned instance."""
    proc = None
    available = False
    with (directory / 'redis.log').open('w') as log:
        try:
            if shutil.which('redis-server'):
                with socket.socket() as reservation:
                    reservation.bind(('127.0.0.1', 6379))
                proc = subprocess.Popen(['redis-server', '--bind', '127.0.0.1', '--port', '6379',
                    '--save', '', '--appendonly', 'no', '--daemonize', 'no', '--dir', str(directory)],
                    stdout=log, stderr=log, start_new_session=True)
                deadline = time.monotonic() + 5
                while proc.poll() is None and time.monotonic() < deadline:
                    try:
                        with socket.create_connection(('127.0.0.1', 6379), timeout=.1):
                            available = True
                            break
                    except OSError:
                        time.sleep(.02)
        except OSError:
            pass
        try:
            yield available
        finally:
            if proc:
                stop_process_group(proc)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--config', type=Path, required=True, help='Configured Linux baseline config.h (read only)')
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test (<build>/rust-prefix/lib/libswoole_rs_ffi.a)')
    parser.add_argument('--build-root', type=Path, required=True, help='Private writable build/cache directory')
    parser.add_argument('--node-modules', type=Path, help='Original JS fixture dependencies; mqtt4.3.8/pino6.14.0 validated')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=40)
    parser.add_argument('--require-all', action='store_true')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.jobs < 1 or args.timeout < 1:
        parser.error('Jobs/timeout must be positive')
    if sys.platform != 'linux':
        parser.error('This recorded native-core build route is Linux-only')
    if subprocess.check_output(['git', '-C', str(args.source), 'rev-parse', 'HEAD'], text=True).strip() != PIN:
        parser.error('Reference pin differs')
    for command in ['cmake', 'c++', 'php-config']:
        if not shutil.which(command):
            parser.error('Missing native build prerequisite: ' + command)
    source = args.source.resolve(strict=True)
    build_root = args.build_root.resolve()
    if build_root == source or source in build_root.parents:
        parser.error('Build root must not modify the pinned reference')
    build_root.mkdir(parents=True, exist_ok=True)
    archive = args.archive.resolve(strict=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
        'scope': 'Unchanged original GTest assertions against full standalone native libraries, reference vs the hybrid build\'s patched tree. Not PHP validation or performance.',
        'require_all': args.require_all, 'engines': [], 'results': [], 'execution_status': 'running'}
    artifacts = args.output.resolve().with_name(args.output.stem + '-artifacts')
    try:
        with tempfile.TemporaryDirectory(prefix='swr-protocols-core-inputs-') as temporary:
            staged = Path(temporary)
            staged_sources(source, staged)
            inventory = [{'name': f'{group}.{name}', 'path': path, 'sha256': sha(staged / path)}
                for path in ASSERTIONS for group, name in re.findall(r'TEST\((\w+),\s*(\w+)\)', (staged / path).read_text())]
            # v6.2.4: redis.* (5, with recv_packet_with_partially_filled_buffer), message_bus.* (2), mqtt.echo.
            if len(inventory) != 8 or len({case['name'] for case in inventory}) != 8:
                raise ValueError('Original eight-case inventory differs')
            report['inventory'] = inventory
            for engine in ['upstream', 'rust']:
                if engine == 'rust':
                    # Every module's patches, as the hybrid build applied them (#796).
                    hybrid_cmake = overlay_hybrid_tree(archive, staged)
                tree = build_root / ('core-' + engine)
                for path in staged.rglob('*'):
                    if path.is_file() and '.git' not in path.relative_to(staged).parts:
                        write_changed(tree / path.relative_to(staged), path.read_bytes())
                write_changed(tree / 'config.h', args.config.read_bytes())
                cmake = (staged / 'CMakeLists.txt').read_text()
                selection = 'file(GLOB_RECURSE core_test_files core-tests/src/*.cpp thirdparty/llhttp/*.c)'
                if cmake.count(selection) != 1:
                    raise ValueError('Original core target selection changed')
                selected = SELECTED
                if engine == 'rust':
                    reply = (Path(__file__).resolve().parent / 'redis_reply.inc').read_text()
                    write_changed(tree / REDIS_REPLY, ('#include "swoole_string.h"\n#include "swoole_redis.h"\n'
                                                       '#include "swoole_rs_protocols.h"\nnamespace swoole {\nnamespace redis {\n'
                                                       + reply + '}  // namespace redis\n}  // namespace swoole\n').encode())
                    selected = [*SELECTED, REDIS_REPLY]
                cmake = cmake.replace(selection, 'set(core_test_files ' + ' '.join(selected) + ')')
                if engine == 'rust':
                    cmake += f'\n{hybrid_cmake}target_link_libraries(lib-swoole "{archive}")\n'
                write_changed(tree / 'CMakeLists.txt', cmake.encode())
                engine_artifacts = artifacts / engine
                engine_artifacts.mkdir(parents=True, exist_ok=True)
                metadata = {'name': engine}
                for case in inventory:
                    if sha(tree / case['path']) != case['sha256']:
                        raise ValueError('Original assertion source changed')
                if engine == 'rust':
                    # Worker's id is Rust state behind get_id() (#735); the assertions are unchanged.
                    for path in ASSERTIONS:
                        text = (tree / path).read_text()
                        write_changed(tree / path, re.sub(r'\bworker->id\b(?!\()', 'worker->get_id()', text).encode())
                for name, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release']),
                        ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'core-tests', '-j' + str(args.jobs)])]:
                    result = run_isolated(command, timeout=600)
                    log = engine_artifacts / (name + '.log')
                    log.write_text(result.stdout + result.stderr)
                    metadata[name] = {'command': command, 'returncode': result.returncode, 'log': str(log)}
                    if result.returncode:
                        report['engines'].append(metadata)
                        raise RuntimeError(engine + ' ' + name + ' failed: ' + str(log))
                binary = tree / 'bin/core-tests'
                binary.parent.mkdir(exist_ok=True)
                shutil.copy2(tree / 'build/bin/core-tests', binary)
                link = tree / 'core-tests/js/node_modules'
                if link.is_symlink():
                    link.unlink()
                elif link.exists():
                    raise ValueError('Unmanaged node_modules directory in private source tree')
                if args.node_modules:
                    link.symlink_to(args.node_modules.resolve(strict=True), target_is_directory=True)
                node = None
                if shutil.which('node'):
                    node = run_isolated(['node', '-e', "console.log(JSON.stringify({node:process.version,mqtt:require('mqtt/package.json').version,pino:require('pino/package.json').version}))"], cwd=tree / 'core-tests/js')
                metadata['node_probe'] = None if node is None else {'returncode': node.returncode, 'stdout': node.stdout, 'stderr': node.stderr}
                with redis_fixture(tree) as redis:
                    missing = {case['name']: ('Owned Redis fixture unavailable' if case['name'] == 'redis.get' and not redis else
                        'Original Node mqtt/pino dependencies unavailable' if case['name'] == 'mqtt.echo' and (node is None or node.returncode) else None) for case in inventory}
                    names = [name for name, reason in missing.items() if reason is None]
                    xml = engine_artifacts / 'results.xml'
                    xml.unlink(missing_ok=True)
                    result = run_isolated([str(binary), '--gtest_filter=' + ':'.join(names), '--gtest_output=xml:' + str(xml)], timeout=args.timeout, cwd=tree)
                    (engine_artifacts / 'run.log').write_text(result.stdout + result.stderr)
                    ledger = {}
                    if xml.is_file():
                        for row in ET.parse(xml).iter('testcase'):
                            name = row.attrib['classname'] + '.' + row.attrib['name']
                            if name in ledger or name not in names:
                                raise ValueError('Duplicate/foreign GTest result')
                            ledger[name] = 'failed' if row.find('failure') is not None or row.find('error') is not None else 'skipped' if row.find('skipped') is not None or row.attrib.get('status') != 'run' else 'passed'
                    for case in inventory:
                        name = case['name']
                        report['results'].append({'engine': engine, **case, 'status': 'unsupported' if missing[name] else ledger.get(name, 'failed'), 'reason': missing[name]})
                    if result.returncode and all(ledger.get(name) == 'passed' for name in names):
                        raise RuntimeError('Unexplained GTest runner failure')
                    metadata['run'] = {'returncode': result.returncode, 'log': str(engine_artifacts / 'run.log'), 'xml': str(xml)}
                report['engines'].append(metadata)
                write_report(args.output, report)
        report['execution_status'] = 'complete'
    except Exception as error:
        report['execution_status'] = 'failed'
        report['error'] = str(error)
    write_report(args.output, report)
    print(json.dumps({'execution_status': report['execution_status'], 'results': report['results'], 'error': report.get('error')}))
    return int(report['execution_status'] != 'complete' or any(row['status'] == 'failed' or args.require_all and row['status'] != 'passed' for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
