#!/usr/bin/env python3
"""Optimized native Buffer append/pop comparison; run only on an idle host."""
import argparse
import contextlib
from datetime import datetime, timezone
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shlex
import statistics
import subprocess
import sys
import tarfile
import tempfile

from baseline import PIN
sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report

WORKLOADS = [
    ('contiguous-4k', 4096, 16384, 0),
    ('contiguous-64k', 65536, 16384, 0),
    ('vector-64k-chunk16k', 65536, 16384, 4),
    ('vector-64k-chunk1k', 65536, 1024, 4),
]


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def selected_build(archive):
    """An SDK archive (tools/build_ext.py --native) and its headers."""
    archive = Path(archive).resolve(strict=True)
    return archive.parents[1] / 'include', archive


def measure(report, binaries, args):
    for name, size, chunk, vectors in WORKLOADS:
        for repeat in range(args.repeat):
            order = list(binaries)
            shift = (repeat // 2) % len(order) if len(order) > 2 else 0
            order = order[shift:] + order[:shift]
            for engine in (order if repeat % 2 == 0 else order[::-1]):
                command = [str(binaries[engine]), str(size), str(chunk), str(vectors), str(args.iterations)]
                if args.peek_front:
                    command.append('peek')
                result = run_isolated(command, timeout=120)
                if result.returncode:
                    report['failure'] = {'engine': engine, 'workload': name, 'repeat': repeat, 'stdout': result.stdout, 'stderr': result.stderr}
                    write_report(args.output, report)
                    raise RuntimeError(f'{engine}/{name}: {result.stderr}')
                trial = json.loads(result.stdout)
                if trial['errors'] or trial['iterations'] != args.iterations or trial['bytes_per_cycle'] != size:
                    raise RuntimeError('Invalid benchmark result: ' + result.stdout)
                report['trials'].append({'engine': engine, 'workload': name, 'repeat': repeat,
                                         'chunk_size': chunk, 'vectors': vectors, **trial})
        medians = {}
        for engine in binaries:
            selected = [trial for trial in report['trials'] if trial['engine'] == engine and trial['workload'] == name]
            medians[engine] = {key: statistics.median(trial[key] for trial in selected)
                               for key in ['ns_per_cycle', 'bytes_per_second']}
        report['comparisons'].append({'workload': name, 'medians': medians,
            'hybrid_vs_upstream_throughput': medians['hybrid']['bytes_per_second'] / medians['upstream']['bytes_per_second'],
            'errors': 0})
        if 'previous' in medians:
            report['comparisons'][-1]['hybrid_vs_previous_throughput'] = medians['hybrid']['bytes_per_second'] / medians['previous']['bytes_per_second']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src')
    parser.add_argument('--archive', type=Path, default=ROOT / 'target/ext/sdk/lib/libswoole_rs_ffi.a')
    parser.add_argument('--previous-archive', type=Path, help='Compare a previous SDK archive in the same trial series')
    parser.add_argument('--iterations', type=int, default=50000)
    parser.add_argument('--repeat', type=int, default=5)
    parser.add_argument('--peek-front', action='store_true', help='Read each DATA front before pop, exercising native consumer cache hits')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--prepare-dir', type=Path, help='Keep immutable optimized consumers and run only untimed checks')
    mode.add_argument('--prepared-report', type=Path, help='Time preserved consumers without compiling')
    parser.add_argument('--compile-only', action='store_true', help='Compile both engines without executing timed trials')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/buffer-benchmark.json'))
    args = parser.parse_args()
    if args.iterations < 1 or args.repeat < 1:
        parser.error('iterations and repeat must be positive')
    if args.prepared_report and args.compile_only:
        parser.error('Prepared measurement cannot use --compile-only')
    if args.prepared_report:
        report = json.loads(args.prepared_report.read_text())
        if not report.get('prepared'):
            parser.error('Need a prepared optimized-consumer report')
        binaries = {name: Path(row['binary']) for name, row in report['engines'].items()}
        for name, binary in binaries.items():
            if sha(binary) != report['engines'][name]['binary_sha256']:
                parser.error('Prepared binary changed: ' + name)
        report.update(trials=[], comparisons=[], iterations=args.iterations, repeat=args.repeat, peek_front=args.peek_front,
                      compile_only=False)
        measure(report, binaries, args)
        write_report(args.output, report)
        print(json.dumps(report['comparisons']))
        return 0
    if args.prepare_dir:
        args.prepare_dir = args.prepare_dir.resolve()
        args.prepare_dir.mkdir(parents=True, exist_ok=True)
        args.compile_only = True
    builds = {'hybrid': selected_build(args.archive)}
    if args.previous_archive:
        builds['previous'] = selected_build(args.previous_archive)
    _, archive = builds['hybrid']
    compiler = shlex.split(os.environ.get('CXX', 'c++'))
    local = Path(__file__).resolve().parent
    report = {
        'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
        'upstream_commit': PIN, 'host': platform.platform(),
        'compiler': subprocess.check_output(compiler + ['--version'], text=True).strip(),
        'archive': str(archive),
        'iterations': args.iterations, 'repeat': args.repeat, 'warmup_cycles': 100, 'peek_front': args.peek_front,
        'scope': 'Native allocation/copy/chunk-pop microbenchmark from the pinned sources and the Cargo build SDK, not PHP server throughput. Correctness preflight and postflight outside timing. Offset zero avoids known upstream offset defects. C++ -O3 without sanitizers; the SDK optimized Rust archive.',
        'engines': {}, 'trials': [], 'comparisons': [],
    }
    context = contextlib.nullcontext(str(args.prepare_dir)) if args.prepare_dir else tempfile.TemporaryDirectory(prefix='swr-buffer-benchmark-')
    with context as temporary:
        binaries = {}
        for engine in ['upstream', *builds]:
            tree = Path(temporary) / engine
            tree.mkdir()
            if engine == 'upstream':
                data = subprocess.check_output(['git', '-C', str(args.source), 'archive', PIN, 'include', 'src/core/buffer.cc'])
                with tarfile.open(fileobj=io.BytesIO(data)) as bundle:
                    for member in bundle:
                        if member.isfile():
                            destination = tree / member.name
                            destination.parent.mkdir(parents=True, exist_ok=True)
                            destination.write_bytes(bundle.extractfile(member).read())
                headers = ['-I' + str(tree / 'include'), '-I' + str(ROOT / 'native/include')]
            else:
                include, archive = builds[engine]
                headers = ['-I' + str(include)]
            binary = tree / 'benchmark'
            command = compiler + ['-std=c++17', '-O3', '-DNDEBUG', '-ffunction-sections', '-fdata-sections',
                                  *headers, *([str(tree / 'src/core/buffer.cc')] if engine == 'upstream' else []), str(local / 'benchmark.cc')]
            if engine in builds:
                command += ['-DSWOOLE_RS_BUFFER', str(archive), '-lm']
                if sys.platform == 'linux':
                    command += ['-ldl', '-lpthread', '-lrt', '-lutil']
            command += ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-o', str(binary)]
            result = run_isolated(command, timeout=60)
            if result.returncode:
                report['compile_failure'] = {'engine': engine, 'command': command, 'stderr': result.stderr}
                write_report(args.output, report)
                raise RuntimeError(result.stderr)
            report['engines'][engine] = {
                'binary': str(binary), 'compile_command': command, 'binary_sha256': sha(binary),
            }
            if engine in builds:
                symbols = subprocess.check_output(['nm', '-g', str(binary)], text=True)
                # The swoole::Buffer members benchmark.cc calls, which crates/buffer/src/cpp.rs
                # defines; upstream's Buffer has no typed_front. The iovec append's off_t differs by OS.
                required = ['_ZN6swoole6BufferC1Ej', '_ZN6swoole6BufferD1Ev', '_ZN6swoole6Buffer6appendEPKcj',
                            '_ZN6swoole6Buffer6appendEPK5iovecm', '_ZN6swoole6Buffer3popEv', '_ZNK6swoole6Buffer11typed_frontEv',
                            '_ZNK6swoole6Buffer6lengthEv', '_ZNK6swoole6Buffer5countEv', '_ZNK6swoole6Buffer5emptyEv']
                if not all(name in symbols for name in required):
                    raise RuntimeError('Prepared native consumer lacks Rust buffer symbols')
                report['engines'][engine]['artifact'] = {'archive': str(archive), 'archive_sha256': sha(archive)}
            binaries[engine] = binary
        if len({sha(binary) for binary in binaries.values()}) != len(binaries):
            raise RuntimeError('Prepared engines unexpectedly have identical binary identities')
        if args.prepare_dir:
            for engine, binary in binaries.items():
                checks = report['engines'][engine]['untimed_checks'] = []
                for name, size, chunk, vectors in WORKLOADS:
                    for peek in [False, True]:
                        command = [str(binary), str(size), str(chunk), str(vectors), 'check', *(['peek'] if peek else [])]
                        checked = run_isolated(command, timeout=30)
                        checks.append({'workload': name, 'peek': peek, 'returncode': checked.returncode,
                                       'stdout': checked.stdout, 'stderr': checked.stderr})
                        if checked.returncode:
                            write_report(args.output, report)
                            raise RuntimeError(checked.stderr)
            report['prepared'] = True
        if args.compile_only:
            report['compile_only'] = True
        else:
            measure(report, binaries, args)
    write_report(args.output, report)
    for item in report['comparisons']:
        print(f'{item["workload"]}: hybrid/upstream={item["hybrid_vs_upstream_throughput"]:.3f}x; errors=0')
    print('Report: ' + args.output)
    return 0


if __name__ == '__main__':
    sys.exit(main())
