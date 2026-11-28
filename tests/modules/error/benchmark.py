#!/usr/bin/env python3
"""Run an immutable prepared error benchmark under a host-wide measurement lease."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import signal
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prepared', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--state', action='store_true', help='Measure fresh-process ignored-error state consumers')
    parser.add_argument('--latency', action='store_true',
                        help='Measure state batch-mean latency quantiles; requires --state and the same exclusive lease')
    args = parser.parse_args()
    if args.latency and not args.state:
        parser.error('--latency requires --state')
    manifest = json.loads((args.prepared / 'manifest.json').read_text())
    name = 'state-benchmark' if args.state else 'benchmark'
    executable = (args.prepared / name).resolve()
    expected = manifest['sha256'][name]
    if not manifest['passed'] or hashlib.sha256(executable.read_bytes()).hexdigest() != expected:
        parser.error('Prepared correctness evidence or executable identity does not match')
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), prepared=manifest, passed=False)
    try:
        command = [str(executable), *(('--latency',) if args.latency else ())]
        process = subprocess.Popen(command, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=30)
        except BaseException:
            # State samples fork: stop the whole private group before releasing
            # a measurement lease, and retain partial diagnostics on failure.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = process.communicate()
            report['execution'] = dict(returncode=process.returncode, stdout=stdout,
                                       stderr=stderr, interrupted=True)
            raise
        result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
        report['execution'] = dict(returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode:
            raise ValueError('Benchmark process failed')
        raw = json.loads(result.stdout)
        if raw['errors'] or result.stderr or hashlib.sha256(executable.read_bytes()).hexdigest() != expected:
            raise ValueError('Benchmark errors, diagnostics or changed executable')
        if args.state:
            if raw.get('timed') is not True or raw.get('latency_mode') is not args.latency:
                raise ValueError('Timed mode does not match the requested state measurement')
        elif raw.get('timed', True) is not True or raw.get('latency_mode', False) is not False:
            raise ValueError('Timed mode does not match the requested measurement')
        summaries = {}
        workloads = manifest['state_benchmark_workloads'] if args.state else manifest.get('benchmark_workloads', ['hot', 'all-known'])
        if {row['workload'] for row in raw['trials']} != set(workloads):
            raise ValueError('Timed workload inventory differs from prepared manifest')
        for workload in workloads:
            trials = [r for r in raw['trials'] if r['workload'] == workload]
            if len(trials) != 7 or any(r['rust_ns'] <= 0 or r['upstream_ns'] <= 0 for r in trials):
                raise ValueError('Incomplete or invalid matched samples')
            if args.latency:
                metrics = ('batch_mean_ns_p50', 'batch_mean_ns_p95', 'batch_mean_ns_p99')
                summaries[workload] = {}
                for metric in metrics:
                    upstream = [r[f'upstream_{metric}'] for r in trials]
                    rust = [r[f'rust_{metric}'] for r in trials]
                    if any(value <= 0 for value in upstream + rust):
                        raise ValueError('Latency samples must be positive')
                    upstream_median = statistics.median(upstream)
                    rust_median = statistics.median(rust)
                    summaries[workload][metric] = dict(upstream_ns=upstream_median,
                                                       rust_ns=rust_median,
                                                       upstream_over_rust=upstream_median / rust_median)
            else:
                summaries[workload] = dict(upstream_ns=statistics.median(r['upstream_ns'] for r in trials),
                                           rust_ns=statistics.median(r['rust_ns'] for r in trials))
                summaries[workload]['throughput_ratio'] = summaries[workload]['upstream_ns'] / summaries[workload]['rust_ns']
        report.update(passed=True, mode='batch_latency' if args.latency else 'whole_sample',
                      raw=raw, summary=summaries)
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(summaries))


if __name__ == '__main__':
    main()
