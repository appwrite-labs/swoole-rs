#!/usr/bin/env python3
"""Compare PHP Frame pack/unpack roundtrips, including payload verification."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, run_isolated, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='NAME=extension; repeat to compare')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--iterations', type=int, default=2000)
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--sizes', type=int, nargs='+', default=[125, 4096, 65536])
    parser.add_argument('--output', default=str(ROOT / 'target/reports/websocket-kernel.json'))
    parser.add_argument('--kind', choices=['binary', 'object', 'close', 'compressed'], default='binary')
    parser.add_argument('--check', action='store_true', help='Verify workload without performance timing')
    args = parser.parse_args()
    if args.iterations <= 0 or args.repeat <= 0 or any(size <= 0 for size in args.sizes):
        parser.error('Iterations, repeat and payload sizes must be positive')
    if args.kind == 'close' and any(size > 125 for size in args.sizes):
        parser.error('Close reason sizes must be <=125; choose --sizes 1 123 125')
    engines = parse_engines(args.engine, args.php, args.php_extension)
    metadata = [engine.metadata() for engine in engines]
    for engine in metadata:
        if 'websocket_codec' not in engine['capabilities']:
            parser.error(f'{engine["name"]} lacks Frame::pack capability')
        if args.kind == 'compressed' and 'zlib' not in engine['capabilities']:
            parser.error(f'{engine["name"]} lacks zlib compression capability')
    entry = Path(__file__).with_suffix('.php')
    revision = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture_output=True, text=True)
    dirty = subprocess.run(['git', 'status', '--porcelain'], cwd=ROOT, capture_output=True, text=True)
    baseline = next((engine.name for engine in engines if engine.name == 'upstream'), engines[0].name)
    report = {
        'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
        'timed': not args.check, 'kind': args.kind,
        'measurement': ('Untimed workload verification' if args.check else
                        'PHP Frame::pack + Frame::unpack + payload/metadata verification; roundtrips/s, not server RPS'),
        'host': {'platform': platform.platform(), 'machine': platform.machine(), 'cpus': os.cpu_count()},
        'revision': revision.stdout.strip() if revision.returncode == 0 else None,
        'dirty': bool(dirty.stdout.strip()), 'engines': metadata,
        'parameters': {'iterations': args.iterations, 'repeat': 1 if args.check else args.repeat, 'payload_sizes': args.sizes,
                       'warmup_roundtrips': 0 if args.check else 200, 'masked': [False, True], 'baseline': baseline},
        'results': [], 'summary': [],
    }
    for size in args.sizes:
        for masked in [False, True]:
            for trial in range(1 if args.check else args.repeat):
                for engine in engines[::1 if trial % 2 == 0 else -1]:
                    result = run_isolated(engine.command(entry, args.iterations, size, int(masked), args.kind, *(['check'] if args.check else [])), timeout=120)
                    if result.returncode:
                        report['failure'] = {'engine': engine.name, 'payload_bytes': size, 'masked': masked,
                                             'trial': trial + 1, 'stdout': result.stdout, 'stderr': result.stderr}
                        write_report(args.output, report)
                        raise RuntimeError(f'{engine.name}: PHP benchmark failed: {result.stderr}')
                    row = json.loads(result.stdout)
                    if (row['errors'] != 0 or row['iterations'] != args.iterations or
                            row['payload_bytes'] != size or row['masked'] != masked or row['kind'] != args.kind or
                            (row.get('verified') != args.iterations if args.check else row.get('elapsed_ns', 0) <= 0)):
                        raise RuntimeError(f'{engine.name}: invalid benchmark result {row}')
                    report['results'].append({'engine': engine.name, 'trial': trial + 1, **row})
                    write_report(args.output, report)
                    if args.check:
                        print(f'{engine.name} {args.kind} size={size} mask={int(masked)}: verified {row["verified"]}', flush=True)
                        continue
                    print(f'{engine.name} size={size} mask={int(masked)} trial={trial + 1}: '
                          f'{row["roundtrips_per_second"]:.0f} roundtrips/s, {row["ns_per_roundtrip"]:.0f} ns/roundtrip', flush=True)
            if args.check:
                continue
            medians = {
                engine.name: statistics.median(row['roundtrips_per_second'] for row in report['results']
                                               if row['engine'] == engine.name and row['payload_bytes'] == size and row['masked'] == masked)
                for engine in engines
            }
            for engine in engines:
                report['summary'].append({
                    'engine': engine.name, 'payload_bytes': size, 'masked': masked,
                    'median_roundtrips_per_second': medians[engine.name],
                    'median_ns_per_roundtrip': statistics.median(row['ns_per_roundtrip'] for row in report['results']
                                                                if row['engine'] == engine.name and row['payload_bytes'] == size and row['masked'] == masked),
                    'throughput_ratio_to_baseline': medians[engine.name] / medians[baseline],
                })
            write_report(args.output, report)
    report['passed'] = True
    write_report(args.output, report)
    print(f'Report: {args.output}')


if __name__ == '__main__':
    main()
