#!/usr/bin/env python3
"""Normal public Logger behavior through separate exact-layout class DSOs.

The native side uses attested production SDK prerequisites and pinned log.cc; the
selected side compiles the hybrid build's log.cc against that build's headers and
config.h. This is module integration, not a rebuilt PHP world.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path[:0] = [str(ROOT), str(Path(__file__).parent)]
from coherent_sdk import stage
from rust_asan import inspect as inspect_rust_asan
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--native-export', type=Path, required=True)
    parser.add_argument('--sdk-ref', required=True, help='Frozen prerequisite SDK source, not current Logger source')
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sanitize', action='store_true', help='Native ASan/UBSan normal consumers')
    parser.add_argument('--rust-asan', action='store_true', help='Require instrumented utility Rust code objects')
    parser.add_argument('--asan-runtime', type=Path, help='Explicit compatible dynamic ASan runtime; requires Rust ASan')
    parser.add_argument('--native-ubsan-only', action='store_true', help='Native UBSan with Rust ASan, for incompatible native/Rust ASan runtimes')
    args = parser.parse_args()
    if args.rust_asan and not args.sanitize:
        parser.error('--rust-asan requires --sanitize')
    if args.asan_runtime and not args.rust_asan:
        parser.error('--asan-runtime requires --rust-asan')
    if args.native_ubsan_only and not args.rust_asan:
        parser.error('--native-ubsan-only requires --rust-asan')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix='logger-state-', dir=args.output.parent)).resolve()
    report = dict(passed=False, work=str(work), scope=__doc__, results=[], identities={},
                  native_sanitized=args.sanitize, rust_sanitized=args.rust_asan,
                  native_sanitizers=(['undefined'] if args.native_ubsan_only else ['address', 'undefined']) if args.sanitize else [])

    def run(command):
        result = run_isolated(command, timeout=60, env={**os.environ, 'LC_ALL': 'C', 'TZ': 'UTC0'})
        report['results'].append(dict(command=command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
        if result.returncode:
            raise ValueError('Command failed; see retained result')
        return result

    try:
        if args.rust_asan:
            report['rust_asan_objects'] = inspect_rust_asan(args.archive)
        sanitizer = ['-fsanitize=' + ('undefined' if args.native_ubsan_only else 'address,undefined'), '-fno-sanitize-recover=all'] if args.sanitize else []
        runtime_link = []
        if args.asan_runtime:
            runtime = args.asan_runtime.resolve(strict=True)
            report['asan_runtime'] = str(runtime)
            if not args.native_ubsan_only:
                sanitizer += ['-fno-sanitize-link-runtime']
            runtime_link = [str(runtime), '-Wl,-rpath,' + str(runtime.parent)]
        manifest = json.loads((ROOT / 'migration/patches/utilities/manifest.json').read_text())
        hybrid, hybrid_flags = hybrid_tree(args.archive)
        for engine in ['native', 'selected']:
            directory = work / engine
            if engine == 'native':
                sdk_receipt = stage(directory, args.native_export, args.source, args.sdk_ref, ROOT)
                headers = ['-I' + str(directory / 'include')]
                source = directory / 'src/core/log.cc'
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_bytes(subprocess.check_output(['git', '-C', str(args.source), 'show', manifest['upstream_commit'] + ':src/core/log.cc']))
            else:
                # The hybrid build's log.cc, headers and config.h.
                directory.mkdir(parents=True)
                shutil.copyfile(hybrid / 'config.h', directory / 'config.h')
                sdk_receipt = dict(hybrid_tree=str(hybrid), flags=hybrid_flags)
                headers = ['-I' + str(directory), *hybrid_flags]
                source = hybrid / 'src/core/log.cc'
                assert b'#define SWOOLE_RS_LOGGER_STATE_ABI 2' in (hybrid / 'include/swoole_log.h').read_bytes()
            common = ['c++', '-std=c++14', '-O2', '-pthread', '-Wall', '-Wextra', '-Werror', '-DHAVE_CONFIG_H', *headers, *shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True))]
            common += sanitizer
            obj = directory / 'log.o'
            run(common + ['-fPIC', '-MD', '-MF', str(directory / 'log.d'), '-c', str(source), '-o', str(obj)])
            library = directory / ('liblogger.dylib' if sys.platform == 'darwin' else 'liblogger.so')
            link = ['-dynamiclib', '-undefined', 'dynamic_lookup'] if sys.platform == 'darwin' else ['-shared']
            run(['c++', *link, *sanitizer, *runtime_link, str(obj), *([str(args.archive.resolve())] if engine == 'selected' else []), '-pthread', *(['-ldl', '-lm'] if sys.platform.startswith('linux') else []), '-o', str(library)])
            consumer = directory / 'public'
            fixture = Path(__file__).with_name('logger_state_public.cc')
            run(common + [str(fixture), str(library), *runtime_link, '-Wl,-export_dynamic' if sys.platform == 'darwin' else '-Wl,--export-dynamic', '-o', str(consumer)])
            imports = run(['nm', '-u', str(consumer)]).stdout
            if not all(symbol in imports for symbol in ['Logger4open', 'Logger5close', 'Logger3put', 'Logger6reopen']):
                raise ValueError('Consumer must import actual class DSO methods')
            if engine == 'selected':
                c_fixture = Path(__file__).with_name('logger_path_c.c')
                c_object = directory / 'path-c.o'
                c_binary = directory / 'path-c'
                run(['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', '-UNDEBUG',
                     *headers, *sanitizer, '-c', str(c_fixture), '-o', str(c_object)])
                run(['c++', *sanitizer, str(c_object), str(args.archive.resolve()), *runtime_link,
                     '-pthread', '-lm', *(['-ldl'] if sys.platform.startswith('linux') else []), '-o', str(c_binary)])
                observations = json.loads(run([str(c_binary)]).stdout)
                if observations != dict(c_path_checks=8, scoped_exports=6):
                    raise ValueError('Missing C11 scoped owner observations')
                report['path_c_consumer'] = dict(observations=observations)
            files = directory / 'files'
            files.mkdir()
            result = run([str(consumer), str(files)])
            values = json.loads(result.stdout)
            if values != dict(public_checks=30, backtrace_effects=2, stream_replacement=True, concurrent_records=1024, reopen_cycles=128, path_values=42, rotation_io=21, long_path_reopen=True):
                raise ValueError('Missing public/effect observations')
            report['identities'][engine] = dict(prerequisite_sdk=sdk_receipt, observations=values)
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({k: report.get(k) for k in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
