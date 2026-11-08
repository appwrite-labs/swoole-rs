#!/usr/bin/env python3
"""Upstream's core-tests ported to Rust (by default core-tests/src/coroutine/{system,hook,socket}.cpp,
tests/modules/hooks/system/; --package names another case package), against the hybrid tree's
libswoole.so (Linux). Builds the hybrid tree's lib-swoole, with --fixture compiled in, links the
case binary to it, and runs each case in its own process group.
--exclude NAME=REASON reports a case `excluded` with its reason; it never counts as passed."""
import argparse
import io
import json
import os
import re
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.build_hybrid import overlay_hybrid_tree  # noqa: E402
from tools.build_upstream import COMMIT as PIN  # noqa: E402
from tools.harness import ROOT, run_isolated, write_report  # noqa: E402

# The cases are system/src/main.rs (with hook.rs and socket.rs); system/Cargo.lock pins its (empty) dependency set for --locked.
PACKAGE = ROOT / 'tests/modules/hooks/system'


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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True, help='Pinned upstream checkout')
    parser.add_argument('--config', type=Path, required=True, help="The hybrid build's configured config.h")
    parser.add_argument('--archive', type=Path, required=True, help='<build>/rust-prefix/lib/libswoole_rs_ffi.a of the hybrid build')
    parser.add_argument('--build-root', type=Path, required=True, help='Private writable build directory')
    parser.add_argument('--package', type=Path, default=PACKAGE, help='The case package (Cargo.toml and a --list binary)')
    parser.add_argument('--fixture', type=Path, help="An owned C++ fixture compiled into lib-swoole for the cases")
    parser.add_argument('--exclude', action='append', default=[], metavar='SUITE.NAME=REASON')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per case')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('the case binary links the Linux hybrid tree')
    excluded = dict(entry.split('=', 1) for entry in args.exclude)
    report = {'scope': __doc__, 'excluded': excluded, 'results': [], 'passed': False}
    logs = args.output.resolve().with_name(args.output.stem + '-artifacts')
    logs.mkdir(parents=True, exist_ok=True)
    try:
        tree = args.build_root.resolve() / 'hybrid'
        with tempfile.TemporaryDirectory(prefix='swr-hooks-system-') as temporary:
            staged = Path(temporary)
            stage(args.source, staged)
            hybrid_cmake = overlay_hybrid_tree(args.archive, staged)
            for path in staged.rglob('*'):
                if path.is_file():
                    write_changed(tree / path.relative_to(staged), path.read_bytes())
        write_changed(tree / 'config.h', args.config.read_bytes())
        cmake = (tree / 'CMakeLists.txt').read_text()
        fixture = f'target_sources(lib-swoole PRIVATE "{args.fixture.resolve()}")\n' if args.fixture else ''
        write_changed(tree / 'CMakeLists.txt', f'{cmake}\n{hybrid_cmake}target_link_libraries(lib-swoole "{args.archive.resolve()}")\n{fixture}'.encode())
        env = {**os.environ, 'SWOOLE_LIB_DIR': str(tree / 'lib'), 'CARGO_TARGET_DIR': str(args.build_root.resolve() / 'cargo')}
        for step, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release',
                                             '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG']),
                              ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'lib-swoole', '-j' + str(args.jobs)]),
                              ('cargo', ['cargo', 'build', '--locked', '--manifest-path', str(args.package / 'Cargo.toml')])]:
            result = run_isolated(command, timeout=900, env=env)
            (logs / (step + '.log')).write_text(result.stdout + result.stderr)
            if result.returncode:
                raise RuntimeError(f'{step} failed: {logs / (step + ".log")}')
        name = re.search(r'^name = "(.+)"$', (args.package / 'Cargo.toml').read_text(), re.M).group(1)
        binary = args.build_root.resolve() / 'cargo/debug' / name
        inventory = subprocess.check_output([str(binary), '--list'], text=True).split()
        if not inventory or len(set(inventory)) != len(inventory) or set(excluded) - set(inventory):
            raise ValueError('Empty or duplicate inventory, or an exclusion names no case')
        for name in inventory:
            status = 'excluded'
            if name not in excluded:
                try:
                    result = run_isolated([str(binary), name], timeout=args.timeout, cwd=tree)
                except subprocess.TimeoutExpired as error:
                    result = subprocess.CompletedProcess(name, 124, '', f'Timed out: {error}')
                (logs / (name + '.log')).write_text(result.stdout + result.stderr)
                status = 'failed' if result.returncode else 'passed'
            report['results'].append({'name': name, 'status': status})
            write_report(args.output, report)
        report['passed'] = all(row['status'] in ('passed', 'excluded') for row in report['results'])
    except Exception as error:
        report['error'] = str(error)
    write_report(args.output, report)
    failed = [row['name'] for row in report['results'] if row['status'] not in ('passed', 'excluded')]
    print(json.dumps({'passed': report['passed'], 'cases': len(report['results']), 'failed': failed, 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
