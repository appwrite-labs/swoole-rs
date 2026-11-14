#!/usr/bin/env python3
"""Compile pinned error.cc and compare it with the hybrid build's error facade, all Rust
(crates/error): swoole_strerror, the ignored-error set, the last-error buffer and throw."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import os
import signal
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
import sys
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.build_upstream import COMMIT as PIN


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/error-differential.json')
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test; it and its headers are the hybrid side')
    parser.add_argument('--allocator-faults', action='store_true', help='Link unchanged Rust compiled with a test allocator instead of the archive and check NOMEM handling')
    parser.add_argument('--sanitize', action='store_true', help='Instrument the native oracle, adapter and driver with ASan/UBSan')
    parser.add_argument('--benchmark', action='store_true', help='Requires an explicit exclusive benchmark lease; seven matched optimized trials')
    parser.add_argument('--prepare-benchmark', type=Path, help='Retain a verified optimized executable without running timed trials')
    args = parser.parse_args()
    if args.allocator_faults and (args.benchmark or args.prepare_benchmark):
        parser.error('Allocator-fault artifacts are test-only, never published archives or benchmarks')
    if (args.benchmark or args.prepare_benchmark) and args.sanitize:
        parser.error('Benchmarks require an optimized unsanitized build')
    if args.prepare_benchmark and args.prepare_benchmark.exists():
        parser.error('Benchmark destination must be new so prepared artifacts remain immutable')
    report = {'upstream_commit': PIN, 'created_utc': datetime.now(timezone.utc).isoformat(),
              'passed': False, 'native_sanitizers': args.sanitize, 'commands': [], 'sha256': {},
              'scope': 'Actual complete pinned error.cc and enum; only swoole.h dependency surface stubbed. '
                       'No PHP/extension/build activation or performance claim.'}
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def run(command, cwd=None):
        # Bound every compiler/probe; terminate its whole private group on timeout/interruption.
        process = subprocess.Popen([str(x) for x in command], cwd=cwd, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = process.communicate(timeout=60)
        except BaseException:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = process.communicate()
            report['commands'].append({'argv': [str(x) for x in command],
                                       'returncode': process.returncode, 'interrupted': True,
                                       'stdout': stdout, 'stderr': stderr})
            raise
        result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
        report['commands'].append({'argv': [str(x) for x in command], 'returncode': result.returncode,
                                   'stdout': result.stdout, 'stderr': result.stderr})
        if result.returncode:
            raise RuntimeError(f'Command failed: {command}')
        return result.stdout

    def pinned(path):
        return run(['git', '-C', args.source.resolve(), 'show', f'{PIN}:{path}'])

    try:
        report['rustc'] = run(['rustc', '--version']).strip()
        report['cxx'] = run(['c++', '--version']).splitlines()[0]
        original = pinned('src/core/error.cc')
        header = pinned('include/swoole_error.h')
        cases = re.findall(r'case (SW_ERROR_\w+):', original)
        names = list(dict.fromkeys(re.findall(r'\bSW_ERROR_\w+\b', header)))
        assert len(cases) == 130 and len(cases) == len(set(cases))
        hybrid_flags = native_flags(args.archive)
        # The scratch source is deliberately outside every worktree, never the reference.
        with tempfile.TemporaryDirectory(prefix='swoole-error-') as temporary:
            work = Path(temporary)
            upstream = work / 'upstream'
            upstream.mkdir()
            (upstream / 'original.cc').write_text(original)
            (upstream / 'swoole_error.h').write_text(header)
            stub = '''#pragma once
#include <cstdio>
#include <cstring>
#include <exception>
#include "swoole_error.h"
const char *swoole_strerror(int);
void swoole_throw_error(int);
void swoole_ignore_error(int);
bool swoole_is_ignored_error(int);
void swoole_clear_last_error_msg();
const char *swoole_get_last_error_msg();
extern char sw_error[1024];
'''
            hybrid_stub = stub.replace('"swoole_error.h"', '"swoole_rs_error.h"').replace(
                'extern char sw_error[1024];', 'extern "C" char *swr_v1_error_buffer(void);\n#define sw_error (swr_v1_error_buffer())')
            # The stub stands in for swoole.h on both sides: with the pinned swoole_error.h in the
            # upstream TU, and with the hybrid build's generated swoole_rs_error.h in the hybrid TUs.
            (upstream / 'swoole.h').write_text(stub)
            (work / 'swoole.h').write_text(hybrid_stub)
            archive = args.archive.resolve()
            report['published_archive'] = str(archive)
            assert archive.is_file(), f'Missing published archive: {archive}'
            if args.allocator_faults:
                rust = ROOT / 'crates/error-ffi/src/lib.rs'
                wrapper = (ROOT / 'tests/modules/error/allocator.rs').read_text()
                wrapper = wrapper.replace('/* ERROR_MODULE */', f'#[path = {json.dumps(str(rust))}] mod error;')
                (work / 'allocator.rs').write_text(wrapper)
                optimized = ['--edition=2024', '-C', 'opt-level=3', '-C', 'panic=abort']
                safe = work / 'libswoole_rs_error.rlib'
                run(['rustc', *optimized, '--crate-name', 'swoole_rs_error', '--crate-type', 'rlib',
                     ROOT / 'crates/error/src/lib.rs', '-o', safe])
                archive = work / 'liberror.a'
                run(['rustc', *optimized, '--crate-name', 'swoole_rs_error_ffi', '--crate-type', 'staticlib',
                     '--cfg', 'feature="upstream-symbols"', '--extern', f'swoole_rs_error={safe}',
                     work / 'allocator.rs', '-o', archive])
            rename = ['swoole_strerror', 'swoole_throw_error', 'swoole_ignore_error',
                      'swoole_is_ignored_error', 'swoole_clear_last_error_msg', 'swoole_get_last_error_msg']
            common = ['-std=c++17', '-O2']
            if args.sanitize:
                common += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
            run(['c++', *common, '-I', upstream, '-Dswoole=reference_swoole', '-Dsw_error=reference_sw_error',
                 *[f'-D{name}=reference_{name}' for name in rename], '-c', upstream / 'original.cc', '-o', work / 'original.o'])
            # The driver uses the hybrid build's headers.
            flags = [*common, '-I', work, *hybrid_flags]
            if args.allocator_faults:
                run(['c++', *flags, ROOT / 'tests/modules/error/allocator.cc',
                     archive, '-o', work / 'allocator-probe'])
                report['allocator_faults'] = json.loads(run([work / 'allocator-probe']))
                report['allocator_archive_scope'] = 'Unchanged production Rust with test-only failing global allocator; not a published PHP archive or Rust-ASan build'
            driver = (ROOT / 'tests/modules/error/driver.cc').read_text()
            driver = driver.replace('/* KNOWN_CASES */', ', '.join(cases))
            driver = driver.replace('/* ALL_ENUMS */', ', '.join(names))
            (work / 'driver.cc').write_text(driver)
            run(['c++', *flags, '-pthread', work / 'driver.cc', work / 'original.o',
                 archive, '-o', work / 'probe'])
            symbols = run(['nm', '-g', work / 'probe'])
            assert re.search(r'\bT\s+_?swr_v1_error_message\b', symbols)
            assert re.search(r'\bT\s+_?_Z15swoole_strerrori\b', symbols), 'swoole_strerror must be Rust'
            report['commands'][-1]['stdout'] = 'swr_v1_error_message defined'
            report['result'] = json.loads(run([work / 'probe']))
            report['native_lines_removed'] = len(original.splitlines())
            if args.benchmark or args.prepare_benchmark:
                report['benchmark_workloads'] = ['all-known', 'hot', 'system', 'unknown']
                benchmark = (ROOT / 'tests/modules/error/benchmark.cc').read_text()
                (work / 'benchmark.cc').write_text(benchmark.replace('/* KNOWN_CASES */', ', '.join(cases)))
                run(['c++', *flags, '-pthread', work / 'benchmark.cc', work / 'original.o',
                     archive, '-o', work / 'benchmark'])
                report['sha256']['benchmark'] = hashlib.sha256((work / 'benchmark').read_bytes()).hexdigest()
                state_source = ROOT / 'tests/modules/error/benchmark-state.cc'
                run(['c++', *flags, '-pthread', state_source, work / 'original.o',
                     archive, '-o', work / 'state-benchmark'])
                report['state_preflight'] = json.loads(run([work / 'state-benchmark', '--check']))
                assert report['state_preflight']['timed'] is False and report['state_preflight']['errors'] == 0
                report['state_benchmark_workloads'] = [row['workload'] for row in report['state_preflight']['trials']]
                report['state_latency_preflight'] = json.loads(run([work / 'state-benchmark', '--check-latency']))
                assert report['state_latency_preflight']['timed'] is False
                assert report['state_latency_preflight']['latency_mode'] is True
                assert report['state_latency_preflight']['errors'] == 0
                assert [row['workload'] for row in report['state_latency_preflight']['trials']] == report['state_benchmark_workloads']
                for row in report['state_latency_preflight']['trials']:
                    assert row['upstream_batch_mean_ns_p50'] == 0
                    assert row['rust_batch_mean_ns_p50'] == 0
                report['sha256']['state-benchmark'] = hashlib.sha256((work / 'state-benchmark').read_bytes()).hexdigest()
                if args.prepare_benchmark:
                    destination = args.prepare_benchmark.resolve()
                    destination.mkdir(parents=True)
                    shutil.copy2(work / 'benchmark', destination / 'benchmark')
                    shutil.copy2(work / 'state-benchmark', destination / 'state-benchmark')
                    report['prepared_benchmark'] = str(destination / 'benchmark')
                    report['passed'] = True
                    (destination / 'manifest.json').write_text(json.dumps(report, indent=2) + '\n')
                if args.benchmark:
                    report['benchmark'] = json.loads(run([work / 'benchmark']))
                    report['state_latency_benchmark'] = json.loads(run([work / 'state-benchmark', '--latency']))
            report['passed'] = True
    finally:
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], **report['result'], 'report': str(args.output)}))


if __name__ == '__main__':
    main()
