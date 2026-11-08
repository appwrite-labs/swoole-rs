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
    'core-tests/src/_lib/process.cpp': [('worker.pid = pid;', 'swr_v1_worker_set_pid(reinterpret_cast<swr_v1_worker_record *>(&worker), pid);')],
    # The hybrid has no swoole_http2.h: the case reads its two constants and crates/http2's settings codec.
    'core-tests/src/server/http_server.cpp': [('#include "swoole_http2.h"\n', '''\
#define SW_HTTP2_PRI_STRING "PRI * HTTP/2.0\\r\\n\\r\\nSM\\r\\n\\r\\n"
#define SW_HTTP2_FRAME_HEADER_SIZE 9
namespace swoole { namespace http2 {
struct Settings { uint32_t header_table_size, enable_push, max_concurrent_streams, init_window_size, max_frame_size, max_header_list_size; };
size_t pack_setting_frame(char *buf, const Settings &settings, bool server_side);
}}
extern "C" swoole::http2::Settings sw_rs_http2_default_settings();
namespace swoole { namespace http2 {
static inline void init_settings(Settings *settings) { *settings = sw_rs_http2_default_settings(); }
}}
'''),
        # The static handler's setters are gone; the cases fill Server's Rust-held string sets directly.
        ('add_static_handler_location(', 'locations.insert('), ('add_static_handler_index_files(', 'http_index_files.insert(')],
    # swoole::realtime_add is Rust's (crates/timer); the hybrid's swoole.h no longer declares it.
    'core-tests/src/os/timer.cpp': [('TEST(timer, realtime_add) {',
                                     'namespace swoole { void realtime_add(timespec *time, int64_t add_msec); }\nTEST(timer, realtime_add) {')],
    # The raw-bytes dispatch_sync overload and the stream listener are crates/process exports, not members.
    'core-tests/src/os/process_pool.cpp': [
        ('TEST(process_pool, tcp_raw) {', 'extern "C" bool swr_v1_process_pool_ipc_send_sync(const swr_v1_process_pool_ipc *, const char *, uint32_t);\n'
                                          'extern "C" void *swr_v1_process_pool_ipc_listener(const swr_v1_process_pool_ipc *);\n'
                                          'TEST(process_pool, tcp_raw) {'),
        ('pool.dispatch_sync(data.str, data.length);', '(swr_v1_process_pool_ipc_send_sync(pool.ipc_, data.str, data.length) ? SW_OK : SW_ERR);'),
        ('pool.stream_info_->socket->shutdown(SHUT_RDWR);',
         'static_cast<swoole::network::Socket *>(swr_v1_process_pool_ipc_listener(pool.ipc_))->shutdown(SHUT_RDWR);')],
    # The global hook table is Rust's, not SwooleG.hooks. Each case runs in its own process, which
    # starts with no hooks and ends in swoole_clean, so the save, reset and restore have nothing to do.
    'core-tests/src/reactor/base.cpp': [
        ('''    void *previous_hook = SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY];
    SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY] = nullptr;
    ON_SCOPE_EXIT {
        delete static_cast<std::list<Callback> *>(SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY]);
        SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY] = previous_hook;
    };
''', ''),
        ('''    ON_SCOPE_EXIT {
        delete static_cast<std::list<Callback> *>(SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_CREATE]);
        delete static_cast<std::list<Callback> *>(SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY]);
        SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_CREATE] = nullptr;
        SwooleG.hooks[SW_GLOBAL_HOOK_ON_REACTOR_DESTROY] = nullptr;
    };
''', '')],
}
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
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per test')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('core-tests build on Linux only')
    excluded = dict(entry.split('=', 1) for entry in args.exclude)
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
                        if old not in text:
                            raise ValueError(f'Test helper changed: {path}')
                        text = text.replace(old, new)
                    write_changed(tree / path, text.encode())
                cmake = (staged / 'CMakeLists.txt').read_text()
                if cmake.count(SELECTION) != 1:
                    raise ValueError('Upstream core-tests target selection changed')
                cmake = cmake.replace(SELECTION, 'file(GLOB core_test_files ' + ' '.join(['core-tests/src/main.cpp', 'core-tests/src/_lib/*.cpp', *args.file]) + ')')
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
