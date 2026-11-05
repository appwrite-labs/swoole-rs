#!/usr/bin/env python3
"""Prepare optimized utility comparisons; run correctness only, never timings."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import uuid
from gate import ROOT, PIN, oracle
from bytes_gate import SIGNATURES, body
from tools.harness import run_isolated, write_report
from tools.build_hybrid import module_patches
from tools.build_upstream import require_build_space

WORKLOADS = [f'hash-{algorithm}-{length}' for algorithm in range(3) for length in [32, 256, 4096]] + [f'lowercase-{length}' for length in [32, 256, 4096]] + ['pretty', 'micros', 'line', 'truncated-line', 'trim-left', 'trim-right', 'find-binary', 'find-casefold', 'find-reverse', *['compare-'+str(i) for i in range(6)], 'partition-before', 'partition-after', 'policy-level', 'policy-label', 'policy-rotation', 'wait-ready', 'wake-ready']


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def prepare_oracles(source, attempt, report):
    def original(path):
        return subprocess.check_output(['git', '-C', str(source), 'show', PIN+':'+path]).decode()
    functions = []
    for name, signatures in SIGNATURES.items():
        content = original(name)
        functions.extend(body(content, signature) for signature in signatures)
    (attempt/'benchmark-bytes.inc').write_text('\n'.join(functions))
    misc = original('src/core/misc.cc')
    wait = misc[misc.index('#ifdef HAVE_FUTEX'):misc.index('/* {{{ DJBX33A')]
    for include in ['#include <linux/futex.h>\n', '#include <sys/syscall.h>\n']:
        wait = wait.replace(include, '')
    (attempt/'benchmark-wait.inc').write_text(wait)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True, help='Optimized ordinary utility or FFI static archive')
    parser.add_argument('--build-manifest', type=Path, required=True, help='Immutable hybrid release build matching the archive')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--config', type=Path, help='Captured native config.h; defaults beside exported build manifest')
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    attempt = args.output.resolve().parent / ('utilities-prepared-' + uuid.uuid4().hex)
    attempt.mkdir()
    report = dict(passed=False, created_utc=datetime.now(timezone.utc).isoformat(), upstream_pin=PIN,
                  scope='Optimized exact pinned utility bodies versus published Rust calls. Correctness preparation only; no elapsed timing, speed, PHP, full logger concurrency/calendar/rotation or activation claim. Native string allocation remains for pretty-name facade.',
                  attempt=str(attempt), platform=platform.platform(), workloads=WORKLOADS, checks=[])
    try:
        require_build_space(ROOT)
        archive = args.archive.resolve(strict=True)
        report['archive_input'] = str(archive); report['archive_sha256'] = sha(archive)
        build = json.loads(args.build_manifest.read_text())
        if (build.get('build_mode') != 'immutable' or build['inputs']['backend'] != 'rust'
                or 'utilities' not in build['inputs']['modules']
                or build['inputs']['upstream_commit'] != PIN
                or build['inputs']['rust_archive_sha256'] != report['archive_sha256']
                or build['inputs']['headers_sha256']['native/utilities/swoole_rs_utilities.h'] != sha(ROOT/'native/utilities/swoole_rs_utilities.h')):
            raise ValueError('Require matching immutable utilities-selected release build/header/pin')
        compilation = build.get('rust_compilation', {})
        profile = compilation.get('artifact', {}).get('profile', {})
        if ('--release' not in compilation.get('command', []) or profile.get('opt_level') not in ['2', '3', 's', 'z']
                or profile.get('debug_assertions') is not False or profile.get('test') is not False):
            raise ValueError('Require observed optimized non-test release archive compilation')
        selected_sources = build['inputs'].get('rust_sources_sha256', {})
        for name, expected in selected_sources.items():
            if name.startswith('crates/utilities/') and sha(ROOT/name) != expected:
                raise ValueError('Selected utility source mismatch: '+name)
        required_sources = {'crates/utilities/Cargo.toml'} | {
            str(path.relative_to(ROOT)) for path in (ROOT/'crates/utilities/src').rglob('*.rs')}
        if {name for name in selected_sources if name.startswith('crates/utilities/')} != required_sources:
            raise ValueError('Require complete current utility source attestation')
        required_patches = {'migration/patches/utilities/'+row['path'] for row in module_patches(ROOT/'migration/patches/utilities')}
        supplied_patches = [row['path'] for row in build['inputs']['patches']
                            if row['path'].startswith('migration/patches/utilities/')]
        if set(supplied_patches) != required_patches or len(supplied_patches) != len(required_patches):
            raise ValueError('Require complete current utility patch attestation without duplicates')
        for patch in build['inputs']['patches']:
            if patch['path'].startswith('migration/patches/utilities/') and sha(ROOT/patch['path']) != patch['sha256']:
                raise ValueError('Selected utility patch mismatch: '+patch['path'])
        captured_config = args.config or args.build_manifest.parent/'config.h'
        shutil.copy2(captured_config, attempt/'config.h')
        (attempt/'benchmark-config.h').write_text('#define HAVE_FUTEX 1\n' if re.search(r'^#define HAVE_FUTEX 1$', captured_config.read_text(), re.M) else '/* native fallback wait configuration */\n')
        report['have_futex'] = 'HAVE_FUTEX' in (attempt/'benchmark-config.h').read_text()
        export = args.build_manifest.parent/'export.json'
        if export.is_file():
            observed = json.loads(export.read_text())
            for name, path in [('build-manifest.json', args.build_manifest), ('libswoole_rs_ffi.a', archive), ('config.h', captured_config)]:
                if sha(path) != observed['files'][name]['sha256']:
                    raise ValueError('Immutable export mismatch: '+name)
            shutil.copy2(export, attempt/'export.json')
            report['selected_source_ref'] = observed['source_ref']
            report['reference_build_attestation'] = observed.get('reference_build_attestation', False)
        prepare_oracles(args.source, attempt, report)
        shutil.copy2(ROOT/'native/utilities/swoole_rs_utilities.h', attempt/'swoole_rs_utilities.h')
        shutil.copy2(args.build_manifest, attempt/'build-manifest.json')
        frozen = attempt/'selected.a'; shutil.copy2(archive, frozen)
        if sha(frozen) != report['archive_sha256']: raise ValueError('Archive changed during capture')
        extracted = attempt/'oracle.inc'; extracted.write_text(oracle(args.source))
        report['binaries'] = []
        for filename, workloads in [('benchmark.cc', WORKLOADS)]:
            driver = attempt/filename; shutil.copy2(Path(__file__).with_name(filename), driver)
            binary = attempt/Path(filename).stem
            command = ['c++', '-std=c++17', '-O3', '-UNDEBUG', '-funsigned-char', '-Wall', '-Wextra', '-Werror', '-Wno-implicit-fallthrough', '-Wno-ignored-qualifiers', '-I'+str(attempt), str(driver), str(frozen), '-lm', '-lpthread', '-o', str(binary)]
            if sys.platform.startswith('linux'): command += ['-ldl', '-lrt', '-lutil']
            require_build_space(attempt)
            compiled = run_isolated(command, timeout=120)
            artifact = dict(workloads=workloads, compile=dict(command=command, returncode=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr))
            report['binaries'].append(artifact)
            if compiled.returncode:
                continue
            symbols = subprocess.run(['nm', '-u', str(binary)], text=True, capture_output=True, check=True)
            artifact['linked_imports'] = symbols.stdout
            if '__asan_' in symbols.stdout or '__ubsan_' in symbols.stdout:
                raise ValueError('Linked comparison uses sanitizer instrumentation')
            artifact['binary'] = str(binary)
            for name in workloads:
                for engine in ['native', 'rust']:
                    result = run_isolated([str(binary), '--check', engine, name, '3'], timeout=30)
                    row = dict(workload=name, engine=engine, returncode=result.returncode, stdout=result.stdout, stderr=result.stderr)
                    report['checks'].append(row)
                    if not result.returncode:
                        try:
                            row['outcome'] = json.loads(result.stdout)
                            if row['outcome']['elapsed_ns'] != 0:
                                raise ValueError('Preparation unexpectedly measured time')
                        except (ValueError, KeyError) as error:
                            row['error'] = str(error)
            artifact['measure_command_template'] = [str(binary), '--measure', 'ENGINE', 'WORKLOAD', 'ITERATIONS']
        report['compiler'] = subprocess.check_output(['c++', '--version'], text=True)
        report['passed'] = (len(report['checks']) == 2*len(WORKLOADS) and
                            all(row['returncode'] == 0 and 'error' not in row for row in report['checks']) and
                            all(row['compile']['returncode'] == 0 for row in report['binaries']))
    except Exception as error:
        report['error'] = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            report['error_stdout'] = str(error.stdout)
            report['error_stderr'] = str(error.stderr)
    write_report(attempt/'report.json', report); write_report(args.output, report)
    print(json.dumps({k: report.get(k) for k in ['passed', 'error', 'attempt']}))
    return int(not report['passed'])


if __name__ == '__main__': sys.exit(main())
