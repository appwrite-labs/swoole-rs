#!/usr/bin/env python3
"""Prepare both release harnesses, then alternate isolated baseline/candidate trials."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
WORKLOADS = {'buffer-1k': 4000, 'buffer-16k': 4000, 'buffer-cold-1k': 4000, 'timer-512': 2000,
             'timer-4096': 250, 'timer-churn-512': 2000}



def source_hashes(root):
    paths = list((root / 'crates').rglob('*.rs')) + list((root / 'crates').rglob('Cargo.toml'))
    paths += [root / name for name in ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml']]
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--repeat', type=int, default=7)
    parser.add_argument('--prepare-only', action='store_true')
    parser.add_argument('--min-ms', type=float, default=100, help='Minimum calibrated duration of the faster engine')
    args = parser.parse_args()
    if args.repeat < 3 or args.min_ms <= 0:
        parser.error('at least three trials and a positive duration required')
    build = ROOT / 'target/storage-bench'
    binaries = {}
    for label, source in [('baseline', args.baseline.resolve()), ('candidate', ROOT)]:
        project = build / label
        project.mkdir(parents=True, exist_ok=True)
        dependencies = '\n'.join(f'swoole-rs-{name} = {{ path = {json.dumps(str(source / "crates" / name))} }}'
                                 for name in ['buffer', 'timer'])
        (project / 'Cargo.toml').write_text('[package]\nname="storage-bench"\nversion="0.0.0"\nedition="2024"\n'
            '[workspace]\n[dependencies]\n' + dependencies + '\n[[bin]]\nname="storage-bench"\npath=' +
            json.dumps(str(ROOT / 'benchmarks/storage.rs')) + '\n[profile.release]\nopt-level=3\npanic="abort"\n')
        subprocess.run(['cargo', 'build', '--release', '--manifest-path', str(project / 'Cargo.toml')], check=True)
        binaries[label] = project / 'target/release/storage-bench'
    if args.prepare_only:
        return
    report = {'execution_status': 'running', 'min_ms': args.min_ms, 'scope': 'Rust public-ABI microbenchmarks; no PHP, sockets or server throughput. '
              'System allocator accounting disabled during timing. Peak requested bytes excludes allocator metadata and RSS. '
              f'{args.repeat} alternating whole-workload trials, warmup before each trial; no CPU pinning.',
              'sources': {'baseline': source_hashes(args.baseline.resolve()), 'candidate': source_hashes(ROOT)},
              'repeat': args.repeat, 'baseline_source': str(args.baseline.resolve()),
              'diff_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=ROOT)).hexdigest(),
              'host': platform.platform(), 'cpu': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip()
              if platform.system() == 'Darwin' else platform.processor(),
              'rustc': subprocess.check_output(['rustc', '-Vv'], text=True),
              'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'harness_sha256': hashlib.sha256((ROOT / 'benchmarks/storage.rs').read_bytes()).hexdigest(),
              'binaries': {k: {'path': str(v), 'sha256': hashlib.sha256(v.read_bytes()).hexdigest()} for k, v in binaries.items()},
              'workloads': {}}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for name, rounds in WORKLOADS.items():
        calibration = {label: json.loads(subprocess.check_output([str(binary), name, str(rounds)], text=True))
                       for label, binary in binaries.items()}
        assert calibration['baseline']['checksum'] == calibration['candidate']['checksum'], name
        initial_rounds = rounds
        fastest_ns = min(row['nanoseconds'] for row in calibration.values())
        rounds = max(rounds, math.ceil(rounds * args.min_ms * 1e6 / fastest_ns))
        row = {'rounds': rounds, 'calibration_rounds': initial_rounds, 'calibration': calibration,
               'trials': [], 'allocation_probe': {}}
        for label, binary in binaries.items():
            row['allocation_probe'][label] = json.loads(subprocess.check_output([str(binary), name, str(rounds), 'alloc'], text=True))
        expected = row['allocation_probe']['baseline']['checksum']
        assert row['allocation_probe']['candidate']['checksum'] == expected, name
        for trial in range(args.repeat):
            order = list(binaries) if trial % 2 == 0 else list(reversed(binaries))
            for label in order:
                result = json.loads(subprocess.check_output([str(binaries[label]), name, str(rounds)], text=True))
                assert result['checksum'] == expected, (name, label)
                row['trials'].append({'engine': label, 'trial': trial, **result})
        row['median_ns'] = {label: statistics.median(r['nanoseconds'] for r in row['trials'] if r['engine'] == label) for label in binaries}
        row['speedup'] = row['median_ns']['baseline'] / row['median_ns']['candidate']
        report['workloads'][name] = row
        args.output.write_text(json.dumps(report, indent=2) + '\n')
        print(f'{name}: {row["speedup"]:.3f}x', flush=True)
    assert source_hashes(ROOT) == report['sources']['candidate'], 'candidate sources changed during measurement'
    assert source_hashes(args.baseline.resolve()) == report['sources']['baseline'], 'baseline sources changed during measurement'
    report['execution_status'] = 'complete'
    args.output.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
