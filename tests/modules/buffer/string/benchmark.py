#!/usr/bin/env python3
"""Time optimized, provenance-checked String consumers on an idle host.

  benchmark.py --prepare DIR --archive A --output P.json   build both consumers into DIR, check them untimed
  benchmark.py --prepared-report P.json [...]              time them
"""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parents[4]))
from tools.harness import ROOT, run_isolated, write_report
from string_build import build, pinned_tree

WORKLOADS = ['append-4k', 'append-cold', 'repeat-4k', 'write-reduce', 'query-4k', 'split-4k', 'split-missing',
             'query-dense', 'query-dense-long', 'append-integer', 'append-integer-cold']


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def prepare(directory, archive, output):
    """Optimized upstream and Rust consumers in directory, each checked untimed on every workload."""
    directory.mkdir(parents=True, exist_ok=True)
    report = {'prepared': False, 'sanitize_cpp_only': False, 'engines': {},
              # Timing refuses to compare consumers built from different benchmark.cc.
              'adapter_sha256': {'benchmark.cc': sha(Path(__file__).with_name('benchmark.cc'))}}
    tree = directory / 'tree'
    pinned_tree(ROOT / 'target/upstream/src', tree)
    for engine in ['upstream', 'rust']:
        binary = directory / engine
        compiled = build(tree, engine, 'benchmark.cc', binary, archive, optimize=True)
        if compiled.returncode:
            print(compiled.stderr, file=sys.stderr)
            return 1
        for workload in WORKLOADS:
            checked = run_isolated([str(binary), workload, 'check'], timeout=30)
            if checked.returncode:
                print(f'{engine} {workload}: {checked.stderr}', file=sys.stderr)
                return 1
        report['engines'][engine] = {'binary': str(binary), 'binary_sha256': sha(binary)}
    report['prepared'] = True
    write_report(output, report)
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepare', type=Path, metavar='DIR', help='Build and check consumers instead of timing them')
    parser.add_argument('--archive', type=Path, help='Rust archive, with --prepare')
    parser.add_argument('--prepared-report', type=Path)
    parser.add_argument('--previous-prepared-report', type=Path, help='Matched previous Rust consumer compiled from the same benchmark source')
    parser.add_argument('--workloads', nargs='+', default=['append-4k', 'append-cold', 'repeat-4k', 'write-reduce', 'query-4k', 'split-4k', 'split-missing'], help='Prepared workload subset for a short combined timing window')
    parser.add_argument('--repeat', type=int, default=9)
    parser.add_argument('--iterations', type=int, default=100000)
    parser.add_argument('--output', default='target/reports/string-benchmark.json')
    args = parser.parse_args()
    if args.prepare:
        if not args.archive:
            parser.error('--prepare needs --archive')
        return prepare(args.prepare.resolve(), args.archive.resolve(strict=True), args.output)
    if not args.prepared_report:
        parser.error('Need --prepare or --prepared-report')
    if args.repeat < 1 or args.iterations < 1: parser.error('Repeat and iterations must be positive')
    prepared = json.loads(args.prepared_report.read_text())
    if not prepared.get('prepared') or prepared.get('sanitize_cpp_only'): parser.error('Need optimized prepared consumers')
    artifacts = dict(prepared['engines'])
    previous = json.loads(args.previous_prepared_report.read_text()) if args.previous_prepared_report else None
    if previous:
        if not previous.get('prepared') or previous.get('sanitize_cpp_only'): parser.error('Previous consumers must be optimized')
        if previous['adapter_sha256']['benchmark.cc'] != prepared['adapter_sha256']['benchmark.cc']: parser.error('Benchmark source differs')
        artifacts['previous'] = previous['engines']['rust']
    for row in artifacts.values():
        if sha(row['binary']) != row['binary_sha256']: parser.error('Consumer binary changed')
    report = {'scope': 'Optimized native String consumer, not PHP throughput. No sanitizers. Existing allocator and retained native dependencies identical. Payload/accounting pre/postflight; timed checksum; rotated/reversed engine order.',
              'prepared': prepared, 'previous_prepared': previous, 'trials': [], 'comparisons': []}
    for workload in args.workloads:
        names = list(artifacts)
        rows = {name: [] for name in names}
        for trial in range(args.repeat):
            order = names[trial % len(names):] + names[:trial % len(names)]
            if trial % 2: order.reverse()
            for engine in order:
                result = run_isolated([artifacts[engine]['binary'], workload, str(args.iterations)], timeout=30)
                row = {'workload': workload, 'engine': engine, 'trial': trial, 'returncode': result.returncode, 'stderr': result.stderr}
                report['trials'].append(row)
                if result.returncode: write_report(args.output, report); print(result.stderr, file=sys.stderr); return 1
                row.update(json.loads(result.stdout)); rows[engine].append(row['ns_per_cycle'])
                if row['errors']: write_report(args.output, report); return 1
        before, after = [statistics.median(rows[name]) for name in ['upstream', 'rust']]
        comparison = {'workload': workload, 'upstream_ns': before, 'rust_ns': after, 'throughput_ratio': before / after}
        if previous:
            old = statistics.median(rows['previous'])
            comparison.update(previous_rust_ns=old, previous_to_current=old / after)
        report['comparisons'].append(comparison)
    report['errors'] = sum(row['errors'] for row in report['trials'])
    write_report(args.output, report)
    print(json.dumps(report['comparisons']))
    return 0
if __name__ == '__main__': sys.exit(main())
