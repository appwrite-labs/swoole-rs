#!/usr/bin/env python3
"""Optimized PHP timer throughput; run only without concurrent builds/tests."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import platform
import re
import shutil
import subprocess
from pathlib import Path
from statistics import median
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.build_upstream import COMMIT
from tools.harness import ROOT, parse_engines, run_isolated, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--repeat', type=int, default=7)
    parser.add_argument('--output', default=str(ROOT / 'target/reports/timer-benchmark.json'))
    args = parser.parse_args()
    if args.repeat < 3:
        parser.error('at least three repeats required')
    engines = parse_engines(args.engine, args.php)
    if 'upstream' not in [engine.name for engine in engines]:
        parser.error('matching upstream control required')
    fixture = Path(__file__).with_suffix('.php')
    php_binary = Path(shutil.which(args.php) or args.php).resolve(strict=True)
    report = {'upstream_commit': COMMIT, 'created_utc': datetime.now(timezone.utc).isoformat(), 'repeat': args.repeat,
              'scope': 'Sequential identical PHP timer workloads, optimized selected artifacts; no production capacity claim. Cancel has no callback latency; dispatch measures registration-to-entry, repeat registration-to-fifth-entry, including scheduler/queue setup. Percentiles are medians of per-trial percentiles, not pooled.',
              'engines': [engine.metadata() for engine in engines], 'builds': {}, 'native_builds': {},
              'host': platform.platform(), 'concurrency': 1,
              'php_binary': str(php_binary),
              'results': [], 'summary': []}
    if len({(row['php'], row['zts']) for row in report['engines']}) != 1:
        raise ValueError('matching PHP build required')
    for engine in engines:
        source = engine.extension.resolve().parents[1]
        makefile = source / 'Makefile'
        text = makefile.read_text()
        flags = dict(re.findall(r'^(CFLAGS|CXXFLAGS) = (.*)$', text, re.MULTILINE))
        if len(flags) != 2 or any(not re.search(r'(?:^| )-O[23s](?: |$)', value) or '-O0' in value for value in flags.values()):
            raise ValueError('optimized native C/C++ build required')
        pin = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
        if pin != report['upstream_commit']:
            raise ValueError('wrong upstream source pin')
        if engine.name == 'upstream':
            subprocess.run(['git', 'diff', '--quiet', 'HEAD', '--'], cwd=source, check=True)
        report['native_builds'][engine.name] = {'source': str(source), 'commit': pin, 'flags': flags}
        manifest = engine.extension.resolve().parents[2] / 'build-manifest.json'
        if manifest.exists():
            data = json.loads(manifest.read_text())
            if data['extension_sha256'] != hashlib.sha256(engine.extension.read_bytes()).hexdigest():
                raise ValueError('extension hash differs from selected manifest')
            report['builds'][engine.name] = data
    if len({json.dumps(row['flags'], sort_keys=True) for row in report['native_builds'].values()}) != 1:
        raise ValueError('matching native optimization flags required')
    try:
        for mode in ['cancel', 'dispatch', 'repeat']:
            for trial in range(args.repeat):
                # Reverse trial order to limit systematic host/order drift.
                for engine in engines[::1 if trial % 2 == 0 else -1]:
                    started_utc = datetime.now(timezone.utc).isoformat()
                    result = run_isolated(engine.command(fixture, mode), timeout=30)
                    finished_utc = datetime.now(timezone.utc).isoformat()
                    report['last_trial'] = {'engine': engine.name, 'mode': mode, 'trial': trial,
                        'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
                    if result.returncode:
                        raise RuntimeError(f'{engine.name}/{mode}: {result.stdout}{result.stderr}')
                    row = json.loads(result.stdout)
                    if row['errors'] or row['operations'] <= 0 or row['seconds'] <= 0:
                        raise ValueError(f'invalid benchmark trial: {row}')
                    report['results'].append({'engine': engine.name, 'trial': trial,
                        'started_utc': started_utc, 'finished_utc': finished_utc, **row})
            base = median(row['operations_per_second'] for row in report['results'] if row['engine'] == 'upstream' and row['mode'] == mode)
            for engine in engines:
                rows = [row for row in report['results'] if row['engine'] == engine.name and row['mode'] == mode]
                throughput = median(row['operations_per_second'] for row in rows)
                report['summary'].append({'engine': engine.name, 'mode': mode, 'median_operations_per_second': throughput,
                    'ratio_vs_upstream': throughput / base, 'errors': sum(row['errors'] for row in rows),
                    'median_trial_callback_elapsed_ms': {p: median(row['callback_elapsed_ms'][p] for row in rows)
                        if rows[0]['callback_elapsed_ms'][p] is not None else None for p in ['p50', 'p95', 'p99']}})
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps(report['summary'], indent=2))
    return int('error' in report)


if __name__ == '__main__':
    sys.exit(main())
