#!/usr/bin/env python3
"""Upstream's core-tests ported to Rust (by default core-tests/src/coroutine/{system,hook,socket}.cpp,
tests/modules/hooks/system/; --package names another case package), on Linux. The pin's lib-swoole
target builds, as a static libswoole.a, what C and C++ is left (the pin's thirdparty and native/'s
Zend-free sources, against the Cargo build SDK's headers, with --fixture compiled in); the case
package links it whole beside the crates (swoole-rs-ffi), built with the SDK archive's features
and profile, so there is one copy of the Rust code. Runs each case in its own process group.
--exclude NAME=REASON reports a case `excluded` with its reason; it never counts as passed."""
import argparse
import io
import json
import os
import re
import shutil
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.build_ext import FFI_FEATURES, native_flags  # noqa: E402
from tools.build_ext import zend_free_native_sources  # noqa: E402
from tools.build_upstream import COMMIT as PIN  # noqa: E402
from tools.harness import ROOT, run_isolated, write_report  # noqa: E402

# The cases are system/src/main.rs (with hook.rs and socket.rs), a workspace member.
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
    parser.add_argument('--archive', type=Path, required=True,
                        help="tools/build_ext.py --native's sdk/lib/libswoole_rs_ffi.a: its headers, features and profile")
    parser.add_argument('--build-root', type=Path, required=True, help='Private writable build directory')
    parser.add_argument('--package', type=Path, default=PACKAGE, help='The case package (Cargo.toml and a --list binary)')
    parser.add_argument('--fixture', type=Path, help="An owned C++ fixture compiled into lib-swoole for the cases")
    parser.add_argument('--exclude', action='append', default=[], metavar='SUITE.NAME=REASON')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--timeout', type=int, default=90, help='Seconds per case')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux':
        parser.error('the cases are Linux core-tests')
    excluded = dict(entry.split('=', 1) for entry in args.exclude)
    report = {'scope': __doc__, 'excluded': excluded, 'results': [], 'passed': False}
    logs = args.output.resolve().with_name(args.output.stem + '-artifacts')
    logs.mkdir(parents=True, exist_ok=True)
    try:
        tree = args.build_root.resolve() / 'tree'
        with tempfile.TemporaryDirectory(prefix='swr-hooks-system-') as temporary:
            staged = Path(temporary)
            stage(args.source, staged)
            # lib-swoole builds native/'s Zend-free sources and upstream's thirdparty ones against
            # the SDK's headers, so upstream's first-party sources and headers go.
            for name in ['include', 'src', 'ext-src']:
                shutil.rmtree(staged / name)
            for name, path in zend_free_native_sources().items():
                write_changed(staged / name, path.read_bytes())
            for path in staged.rglob('*'):
                if path.is_file():
                    write_changed(tree / path.relative_to(staged), path.read_bytes())
        flags = native_flags(args.archive)
        include = flags[-1][2:]
        write_changed(tree / 'config.h', (Path(include) / 'config.h').read_bytes())
        # add_definitions and include_directories reach targets defined before them; add_compile_options does not.
        native_cmake = f'add_definitions({" ".join(flags[:-1])})\ninclude_directories("{include}")\n'
        cmake = (tree / 'CMakeLists.txt').read_text().replace('add_library(lib-swoole SHARED', 'add_library(lib-swoole STATIC')
        fixture = f'target_sources(lib-swoole PRIVATE "{args.fixture.resolve()}")\n' if args.fixture else ''
        write_changed(tree / 'CMakeLists.txt', f'{cmake}\n{native_cmake}{fixture}'.encode())
        # The archive's build (tools/build_ext.py's manifest): the crates build as it did, and reuse its objects.
        manifest = json.loads((args.archive.resolve().parents[2] / 'build-manifest.json').read_text())
        features = [f'swoole-rs-ffi/{name}' for name in manifest['features'] if name in FFI_FEATURES]
        env = {**os.environ, 'SWOOLE_LIB_DIR': str(tree / 'lib'), 'PHP_CONFIG': os.environ.get('PHP_CONFIG', 'php-config')}
        package = re.search(r'^name = "(.+)"$', (args.package / 'Cargo.toml').read_text(), re.M).group(1)
        for step, command in [('configure', ['cmake', '-S', str(tree), '-B', str(tree / 'build'), '-Dphp_dir=/usr', '-DCMAKE_BUILD_TYPE=Release',
                                             '-DCMAKE_POSITION_INDEPENDENT_CODE=ON',
                                             '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -UNDEBUG', '-DCMAKE_C_FLAGS_RELEASE=-O2 -UNDEBUG']),
                              ('build', ['cmake', '--build', str(tree / 'build'), '--target', 'lib-swoole', '-j' + str(args.jobs)]),
                              ('cargo', ['cargo', 'build', '--locked', '-p', package, '--profile', manifest['profile'],
                                         *(['--features', ','.join(features)] if features else []),
                                         '--message-format=json-render-diagnostics'])]:
            result = run_isolated(command, timeout=1800, env=env, cwd=ROOT)
            (logs / (step + '.log')).write_text(result.stdout + result.stderr)
            if result.returncode:
                # The tail names the error in the report, which is all a CI artifact keeps.
                tail = '\n'.join((result.stdout + result.stderr).splitlines()[-20:])
                raise RuntimeError(f'{step} failed: {logs / (step + ".log")}\n{tail}')
        binary, = {message['executable'] for line in result.stdout.splitlines() if line.startswith('{')
                   for message in [json.loads(line)] if message.get('executable')}
        inventory = subprocess.check_output([binary, '--list'], text=True).split()
        if not inventory or len(set(inventory)) != len(inventory) or set(excluded) - set(inventory):
            raise ValueError('Empty or duplicate inventory, or an exclusion names no case')
        for name in inventory:
            status = 'excluded'
            if name not in excluded:
                try:
                    result = run_isolated([binary, name], timeout=args.timeout, cwd=tree)
                except subprocess.TimeoutExpired as error:
                    result = subprocess.CompletedProcess(name, 124, '', f'Timed out: {error}')
                (logs / (name + '.log')).write_text(f'{result.stdout}{result.stderr}\nexit {result.returncode}\n')
                status = 'failed' if result.returncode else 'passed'
            report['results'].append({'name': name, 'status': status})
            write_report(args.output, report)
        report['passed'] = all(row['status'] in ('passed', 'excluded') for row in report['results'])
    except Exception as error:
        report['error'] = str(error)
    write_report(args.output, report)
    failed = [row['name'] for row in report['results'] if row['status'] not in ('passed', 'excluded')]
    # A failed case's own output (its assertion, its exit), which the phase's log tail keeps;
    # a sanitizer writes its reports to its log_path instead (tools/asan.py).
    for name in failed:
        lines = [line for line in (logs / (name + '.log')).read_text(errors='replace').splitlines() if line.strip()]
        print(f'--- {name}', *lines[-8:], sep='\n')
    print(json.dumps({'passed': report['passed'], 'cases': len(report['results']), 'failed': failed, 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
