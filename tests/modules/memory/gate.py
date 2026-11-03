#!/usr/bin/env python3
"""Pinned native versus Rust-backed collection facades, real runtime dependencies."""
import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.build_hybrid import hybrid_tree, module_patches

from tools.build_upstream import COMMIT as PIN
SOURCES = ['src/core/heap.cc', 'src/core/channel.cc', 'src/memory/fixed_pool.cc', 'src/memory/global_memory.cc', 'src/memory/ring_buffer.cc', 'src/memory/shared_memory.cc', 'src/memory/table.cc']


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src')
    parser.add_argument('--extension', type=Path, required=True, help='The hybrid extension: the Rust probe runs under it')
    # Each probe resolves SwooleG, the allocators, Mutex and Pipe from the extension it is
    # loaded beside, so that extension must share the headers the probe compiled against.
    parser.add_argument('--reference-extension', type=Path, required=True, help='The pinned extension: the upstream probe runs under it')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--sanitize', action='store_true')
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test; its patched tree supplies the Rust side')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/memory-gate.json'))
    parser.add_argument('--prepare-benchmark', type=Path, help='Save optimized unsanitized binaries and hash-checked timing recipe after correctness passes; does not measure')
    args = parser.parse_args()
    if args.prepare_benchmark and args.sanitize:
        parser.error('Benchmark preparation requires an unsanitized optimized build')
    prepared = {}
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'passed': False, 'upstream_commit': PIN,
              'scope': 'Original native algorithms versus actual Rust-backed fixed/global/ring pools, heap, channel, mapping and table facades; native runtime allocation/locking/pipe dependencies supplied by the extension built from the same headers (pinned for upstream, hybrid for rust). New assertions, not original GoogleTests. C++ sanitizers instrument only the Rust-backed native probe/facades, not Rust or PHP.',
              'commands': [], 'results': []}
    try:
        if sys.platform != 'darwin' and not sys.platform.startswith('linux'):
            raise ValueError('Supported native hosts: macOS and Linux')
        linux = sys.platform.startswith('linux')
        extensions = {'upstream': args.reference_extension.resolve(strict=True), 'rust': args.extension.resolve(strict=True)}
        if subprocess.check_output(['git', '-C', str(args.source), 'rev-parse', 'HEAD'], text=True).strip() != PIN:
            raise ValueError('Reference pin mismatch')
        report['extensions'] = {variant: str(path) for variant, path in extensions.items()}
        owned = [*sorted((ROOT / 'crates/memory').rglob('*.rs')),
                 *sorted(Path(__file__).parent.glob('*.rs')), *sorted((ROOT / 'native/memory').glob('*.h')),
                 Path(__file__), Path(__file__).with_name('probe.cc'), Path(__file__).with_name('benchmark.cc')]
        source_hashes = {str(path.relative_to(ROOT)): sha(path) for path in owned}
        def run(command, timeout=90, env=None):
            result = run_isolated(command, cwd=ROOT, timeout=timeout, env=env)
            report['commands'].append({'argv': command, 'exit_code': result.returncode,
                                       'stdout': result.stdout, 'stderr': result.stderr})
            if result.returncode:
                raise RuntimeError(f'command failed ({result.returncode}): {result.stdout}{result.stderr}')
            return result
        with tempfile.TemporaryDirectory(prefix='swr-memory-gate-') as temporary:
            directory = Path(temporary)
            archive = args.archive.resolve(strict=True)
            report['rust_tests'] = 'not run: prebuilt archive selected'
            report['archive_source_identity'] = 'caller must verify prebuilt archive/source correspondence'
            report['archive'] = str(archive)
            hybrid, hybrid_flags = hybrid_tree(archive)
            report['hybrid_tree'] = str(hybrid)
            overlay = module_patches(ROOT / 'migration/patches/memory')
            report['patch_manifest'] = overlay
            raw = subprocess.check_output(['git', '-C', str(args.source), 'archive', PIN, 'include', *SOURCES])
            with tarfile.open(fileobj=io.BytesIO(raw)) as bundle:
                for member in bundle:
                    if member.isfile():
                        path = directory / 'upstream' / member.name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(bundle.extractfile(member).read())
            for variant in ['upstream', 'rust']:
                tree = directory / variant
                tree.mkdir(exist_ok=True)
                # Upstream builds the pinned tree; rust builds the hybrid build's patched tree and flags.
                sources, headers = (tree, ['-I' + str(tree / 'include')]) if variant == 'upstream' else (hybrid, hybrid_flags)
                if variant == 'rust':
                    # Preserve every original assertion expression/count. Rust status
                    # checks add validation; they cannot substitute for native witnesses.
                    assertions = {}
                    for source in sorted({*SOURCES, *[path for patch in overlay for path in patch['upstream_paths']]}):
                        def expressions(path):
                            return Counter(re.sub(r'\s+', '', value) for value in re.findall(r'\bassert\s*\((.*?)\);', path.read_text(), re.S))
                        original = expressions(directory / 'upstream' / source)
                        selected = expressions(hybrid / source)
                        missing = original - selected
                        if missing:
                            raise ValueError(f'Original assertions removed from {source}: {dict(missing)}')
                        assertions[source] = sum(original.values())
                    report['native_assertions_preserved'] = assertions
                compiler = shlex.split(os.environ.get('CXX', 'c++'))
                if variant == 'rust':
                    guard = tree / 'guard.cc'
                    guard.write_text('#include "swoole_heap.h"\n')
                    rejected = run_isolated(compiler + ['-std=c++17', '-I' + str(hybrid / 'include'), '-fsyntax-only', str(guard)], cwd=ROOT, timeout=15)
                    report['feature_guard'] = {'exit_code': rejected.returncode, 'stderr': rejected.stderr}
                    if rejected.returncode == 0 or 'Memory overlay requires --enable-swoole-rs-memory' not in rejected.stderr:
                        raise ValueError('Selected overlay did not reject disabled memory feature')
                    configured = tree / 'configured-memory'
                    configured.mkdir()
                    (configured / 'config.h').write_text('#define SWOOLE_RS_MEMORY 1\n')
                    command = compiler + ['-std=c++17', '-DHAVE_CONFIG_H', '-I' + str(configured),
                        *(flag for flag in hybrid_flags if flag.startswith('-I')), '-fsyntax-only', str(guard)]
                    accepted = run_isolated(command, cwd=ROOT, timeout=15)
                    report['configured_feature_guard'] = {'command': command, 'exit_code': accepted.returncode, 'stderr': accepted.stderr}
                    if accepted.returncode:
                        raise ValueError('Heap header failed when memory feature is supplied only by config.h')
                binary = tree / ('probe.so' if linux else 'probe.dylib')
                command = compiler + ['-std=c++17', '-O2', '-g', '-fvisibility=hidden', *headers]
                command += shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True))
                command += ['-shared', '-fPIC', '-Wl,-Bsymbolic'] if linux else ['-dynamiclib', '-undefined', 'dynamic_lookup']
                command += [*[str(sources / source) for source in SOURCES], str(Path(__file__).with_name('probe.cc')), str(Path(__file__).with_name('benchmark.cc'))]
                environment = dict(os.environ)
                if variant == 'rust':
                    command.append(str(archive))
                    if linux:
                        command += ['-ldl', '-lpthread', '-lrt', '-lutil', '-lm']
                    if args.sanitize:
                        command += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
                        environment['ASAN_OPTIONS'] = 'detect_leaks=0:halt_on_error=1:abort_on_error=0:exitcode=86'
                        environment['UBSAN_OPTIONS'] = 'halt_on_error=1:print_stacktrace=1:exitcode=87'
                        if not linux:
                            runtime = subprocess.check_output(compiler + ['--print-file-name=libclang_rt.asan_osx_dynamic.dylib'], text=True).strip()
                            environment['DYLD_INSERT_LIBRARIES'] = runtime
                run(command + ['-o', str(binary)])
                php_extensions = args.php_extension or (['ffi'] if linux else [])
                command = [args.php, '-n', *[arg for module in php_extensions for arg in ['-d', 'extension=' + module]],
                           '-d', 'extension=' + str(extensions[variant]), '-d', 'ffi.enable=1', '-r',
                           '$f=FFI::cdef("int swr_memory_probe(void);", $argv[1]); exit($f->swr_memory_probe());', str(binary)]
                result = run(command, timeout=30, env=environment)
                observations = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                if len(observations) != 1 or not observations[0].get('passed'):
                    raise ValueError('Missing unique native observations')
                preflight = command.copy()
                preflight[preflight.index('-r') + 1] = '$f=FFI::cdef("int swr_memory_construction_check(void);", $argv[1]); exit($f->swr_memory_construction_check());'
                checked = run(preflight, timeout=15, env=environment)
                values = [json.loads(line) for line in checked.stdout.splitlines() if line.startswith('{')]
                if len(values) != 1 or values[0] != {'passed': True, 'owned_constructions': 5,
                        'borrowed_constructions': 5, 'checksum': 900, 'timed': False}:
                    raise ValueError('Missing exact untimed constructor observations')
                report.setdefault('constructor_preflight', {})[variant] = values[0]
                column_check = command.copy()
                column_check[column_check.index('-r') + 1] = '$f=FFI::cdef("int swr_memory_columns_check(void);", $argv[1]); exit($f->swr_memory_columns_check());'
                checked_columns = run(column_check, timeout=15, env=environment)
                column_values = [json.loads(line) for line in checked_columns.stdout.splitlines() if line.startswith('{')]
                if len(column_values) != 1 or column_values[0] != {'passed': True, 'schemas': 3, 'columns_per_schema': 17,
                        'cursor_columns': 65, 'cursor_passes': 5, 'checksum': 11184, 'name_schemas': 2, 'name_passes': 2, 'name_checksum': 12300970927948216643, 'timed': False}:
                    raise ValueError('Missing exact untimed column lifecycle/traversal observations')
                report.setdefault('column_preflight', {})[variant] = column_values[0]
                report['results'].append({'variant': variant, 'observations': observations[0]})
                if args.prepare_benchmark:
                    destination = args.prepare_benchmark.resolve()
                    destination.mkdir(parents=True, exist_ok=True)
                    stored = destination / (variant + binary.suffix)
                    shutil.copy2(binary, stored)
                    timing_command = command.copy()
                    timing_command[-1] = str(stored)
                    timing_command[timing_command.index('-r') + 1] = '$f=FFI::cdef("int swr_memory_benchmark(unsigned int);", $argv[1]); exit($f->swr_memory_benchmark((int)$argv[2]));'
                    prepared[variant] = {'command': timing_command, 'binary': str(stored), 'sha256': sha(stored),
                                         'extension': str(extensions[variant]), 'extension_sha256': sha(extensions[variant])}
                if variant == 'rust':
                    # Shrinking realloc is outside the compared domain; Rust-only.
                    additional = []
                    for function in ['swr_memory_mapping_probe', 'swr_memory_odd_table_probe']:
                        targeted = command.copy()
                        targeted[targeted.index('-r') + 1] = '$f=FFI::cdef("int ' + function + '(void);", $argv[1]); exit($f->' + function + '());'
                        regression = run(targeted, timeout=15, env=environment)
                        values = [json.loads(line) for line in regression.stdout.splitlines() if line.startswith('{')]
                        if len(values) != 1 or not values[0].get('passed'):
                            raise ValueError('Missing bounded safety observation: ' + function)
                        additional.extend(values)
                    report['additional_rust_checks'] = additional
                    report['uncompared_behavior'] = ('Shrinking realloc copies min(old/new payload) and is checked on Rust only; '
                        'native odd-row objects use aligned physical storage with unchanged legacy logical byte accounting. '
                        'Both are Rust-only witnesses.')

            report['passed'] = report['results'][0]['observations'] == report['results'][1]['observations']
            if report['passed'] and args.prepare_benchmark:
                # Freeze the exact build inputs so later source edits need not
                # wait for a timing lease. Reject changes during preparation.
                frozen_hashes = dict(source_hashes)
                for patch in overlay:
                    frozen_hashes['migration/patches/memory/' + patch['path']] = patch['sha256']
                snapshot = args.prepare_benchmark.resolve() / 'source'
                for relative, digest in frozen_hashes.items():
                    stored = snapshot / relative
                    stored.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(ROOT / relative, stored)
                    if sha(stored) != digest:
                        raise ValueError(f'Source changed during preparation: {relative}')
                report['source_snapshot'] = str(snapshot)
                write_report(args.prepare_benchmark / 'manifest.json', {'source_snapshot': str(snapshot), 'frozen_sha256': frozen_hashes, 'upstream_commit': PIN, 'variants': prepared, 'correctness': report['results'], 'timing_cases': ['fixed_ns', 'heap_ns', 'channel_ns', 'ring_ns', 'global_ns', 'table_ns', 'table_clear8_ns', 'table_clear33_ns', 'fixed_construct_owned_ns', 'fixed_construct_borrowed_ns', 'table_schema17_ns', 'table_columns65_ns', 'table_names65_ns', 'table_long_names65_ns'], 'scope': 'Optimized native facades, C++ -O2 and release Rust, no sanitizers. Timing requires exclusive integrator lease; PHP startup excluded. Existing pool operations exclude setup; explicit fixed_construct cases include arena construction/destruction and owned native allocation/free. Column schema17 includes factory/unique declarations/one traversal/drain for min(loops,256) rounds; columns65 excludes setup/teardown. Names65 workloads include actual cursor plus Native borrowed names vs selected scratch copy/resize and whole-byte consumer hashing; short and80byte embedded-NUL prefixes, scratch lifetime per row, setup/teardown excluded. Typed TableV2 selected versus legacy Native Table. No PHP/original-suite parity or full-workload performance claim.'})
            if not report['passed']:
                report['error'] = 'Native and Rust traces differ'
    except Exception as error:
        report['passed'] = False
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({'passed': report['passed'], 'results': report['results'], 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
