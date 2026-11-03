#!/usr/bin/env python3
"""Measure already-prepared native facades during an exclusive benchmark lease."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import sys
from datetime import datetime, timezone

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepared', type=Path, required=True)
    parser.add_argument('--trials', type=int, default=7)
    parser.add_argument('--loops', type=int, default=100000)
    parser.add_argument('--output', required=True)
    parser.add_argument('--verify-only', action='store_true', help='Check frozen inputs/artifacts without running any timing workload')
    args = parser.parse_args()
    if not 3 <= args.trials <= 15 or not 1 <= args.loops <= 1000000:
        parser.error('Require 3–15 trials and 1–1000000 loops')
    report = {'completed': False, 'created_utc': datetime.now(timezone.utc).isoformat(), 'trials': [], 'errors': []}
    try:
        manifest_path = args.prepared / 'manifest.json'
        manifest = json.loads(manifest_path.read_text())
        report['preparation'] = manifest
        snapshot = Path(manifest.get('source_snapshot', ROOT))
        if 'frozen_sha256' in manifest:
            for path, digest in manifest['frozen_sha256'].items():
                if sha(snapshot / path) != digest:
                    raise ValueError(f'Frozen build input changed: {path}')
        else:
            raise ValueError('Preparation predates frozen inputs; prepare again')
        for variant in manifest['variants'].values():
            if sha(variant['binary']) != variant['sha256']:
                raise ValueError('Prepared binary changed')
            if sha(variant['extension']) != variant['extension_sha256']:
                raise ValueError('Extension changed since preparation')
        report['verified'] = True
        if args.verify_only:
            write_report(args.output, report)
            print(json.dumps({'verified': True, 'completed': False, 'measured': False}))
            return 0
        for trial in range(args.trials):
            # Reverse pair order on alternating trials to reduce consistent order bias.
            for name in (['upstream', 'rust'] if trial % 2 == 0 else ['rust', 'upstream']):
                command = manifest['variants'][name]['command'] + [str(args.loops)]
                result = run_isolated(command, cwd=ROOT, timeout=10)
                item = {'trial': trial, 'variant': name, 'argv': command, 'exit_code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
                report['trials'].append(item)
                if result.returncode:
                    raise RuntimeError('Native timing command failed')
                values = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
                if len(values) != 1 or values[0].get('loops') != args.loops:
                    raise ValueError('Missing unique bounded timing observation')
                item['observation'] = values[0]
        checksums = {item['observation']['checksum'] for item in report['trials']}
        if len(checksums) != 1:
            raise ValueError('Timing workloads produced different checksums')
        cases = [key for key in report['trials'][0]['observation'] if key.endswith('_ns')]
        expected_cases = set(manifest.get('timing_cases', ['fixed_ns', 'heap_ns', 'channel_ns', 'ring_ns', 'global_ns', 'table_ns']))
        if set(cases) != expected_cases or any(item['observation'][case] <= 0 for item in report['trials'] for case in cases):
            raise ValueError('Incomplete or nonpositive timing cases')
        report['medians_ns'] = {name: {case: statistics.median(item['observation'][case] for item in report['trials'] if item['variant'] == name) for case in cases} for name in ['upstream', 'rust']}
        report['rust_over_upstream'] = {case: report['medians_ns']['rust'][case] / report['medians_ns']['upstream'][case] for case in cases}
        report['completed'] = True
    except Exception as error:
        report['errors'].append(f'{type(error).__name__}: {error}')
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['completed', 'errors', 'rust_over_upstream']}))
    return int(not report['completed'])


if __name__ == '__main__':
    sys.exit(main())
