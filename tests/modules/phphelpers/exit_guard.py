#!/usr/bin/env python3
"""Require selected production escape scopes to drop Rust owners; retain Native baseline observations."""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report
from tools.ext_identity import attest


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php', required=True)
    p.add_argument('--php-config', required=True)
    p.add_argument('--engine', action='append', required=True)
    p.add_argument('--build-manifest', type=Path, required=True, help="tools/build_ext.py's")
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    output = a.output.resolve()
    work = output.parent / (output.stem + '-files')
    work.mkdir(parents=True, exist_ok=False)
    report = dict(passed=False, results=[], work=str(work), scope=__doc__)
    try:
        engines = parse_engines(a.engine, a.php)
        metadata = [engine.metadata() for engine in engines]
        report['engines'] = metadata
        if len(engines) != 2 or sorted(bool(row['rust_feature_bits']) for row in metadata) != [False, True]:
            raise ValueError('Require one Native and one selected production engine')
        if {row['zts'] for row in metadata} != {1} or len({row['php'] for row in metadata}) != 1:
            raise ValueError('Require a matching actual ZTS pair')
        selected = next(row for row in metadata if row['rust_feature_bits'])
        report['attestation'] = attest(a.build_manifest, selected, 134217728)
        sources = {name: Path(__file__).with_suffix(suffix) for name, suffix in
                   [('rust', '.rs'), ('cpp', '.cc'), ('php', '.php')]}
        for path in sources.values():
            (work / path.name).write_bytes(path.read_bytes())
        report['sources'] = {name: sha(path) for name, path in sources.items()}
        # The guard header crates/ext compiled into the attested build.
        header = ROOT / 'native/corebase/swoole_rs_zend_guard.h'
        (work / header.name).write_bytes(header.read_bytes())
        report['header_sha256'] = sha(work / header.name)
        library = work / 'libexit_guard_owner.a'
        command = ['rustc', '--edition=2024', '--crate-type=staticlib', '-Copt-level=3', '-Cpanic=abort',
                   str(work / sources['rust'].name), '-o', str(library)]
        result = run_isolated(command, timeout=120)
        report['rust_compile'] = dict(command=command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
        write_report(output, report)
        if result.returncode:
            raise ValueError('Rust owner witness did not compile')
        includes = shlex.split(subprocess.check_output([a.php_config, '--includes'], text=True))
        helper = work / 'exit_guard.so'
        command = ['c++', '-std=c++17', '-O2', '-shared', '-fPIC', '-DZEND_ENABLE_STATIC_TSRMLS_CACHE=1',
                   *includes, '-I' + str(work), str(work / sources['cpp'].name), str(library),
                   '-lpthread', '-ldl', '-lm', '-o', str(helper)]
        result = run_isolated(command, timeout=120)
        report['cpp_compile'] = dict(command=command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
        write_report(output, report)
        if result.returncode:
            raise ValueError('Actual PHP SDK owner witness did not compile')
        command14 = [*command]
        command14[1] = '-std=c++14'
        command14[-1] = str(work / 'exit_guard-c++14.so')
        result14 = run_isolated(command14, timeout=120)
        report['cpp14_compile'] = dict(command=command14, exit=result14.returncode,
                                       stdout=result14.stdout, stderr=result14.stderr)
        write_report(output, report)
        if result14.returncode:
            raise ValueError('Actual C++14 PHP SDK owner witness did not compile')
        report['helper_sha256'] = sha(helper)
        selected_engine = next(engine for engine, identity in zip(engines, metadata) if identity['rust_feature_bits'])
        command = selected_engine.command('-d', 'extension=' + str(helper), work / sources['php'].name, 'cpp-probe')
        result = run_isolated(command, timeout=30)
        report['native_cpp_cleanup'] = dict(command=list(map(str, command)), exit=result.returncode,
                                            stdout=result.stdout, stderr=result.stderr,
                                            passed=result.returncode == 0 and result.stdout == 'cpp-probe: ok\n' and not result.stderr)
        write_report(output, report)
        modes = ['return', 'exception', 'custom', 'exit', 'nested', 'coroutine', 'error',
                 'destructor', 'serialize', 'unserialize', 'values', 'shared-set', 'shared-queue', 'fatal', 'cancel', 'cancel-throw']
        for mode in modes:
            expected = json.dumps([mode, 0 if mode in ['return', 'exception'] else 255 if mode == 'fatal' else 235,
                                   int(mode in ['return', 'exception'])], separators=(',', ':')) + '\n'
            for engine, identity in zip(engines, metadata):
                args = ['-d', 'extension=' + str(helper)] if identity['rust_feature_bits'] else []
                command = engine.command(*args, work / sources['php'].name, mode)
                try:
                    result = run_isolated(command, timeout=30)
                    row = dict(engine=engine.name, mode=mode, command=list(map(str, command)),
                               exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                               expected_stdout=expected,
                               passed=result.returncode == 0 and result.stdout == expected and not result.stderr)
                except subprocess.TimeoutExpired as error:
                    decode = lambda value: value.decode(errors='replace') if isinstance(value, bytes) else value or ''
                    row = dict(engine=engine.name, mode=mode, command=list(map(str, command)),
                               exit=None, timed_out=True, stdout=decode(error.stdout), stderr=decode(error.stderr),
                               expected_stdout=expected, passed=False)
                report['results'].append(row)
                write_report(output, report)
        if sha(helper) != report['helper_sha256']:
            raise ValueError('Native owner witness bytes changed')
        if any(sha(engine.extension) != identity['sha256'] for engine, identity in zip(engines, metadata)):
            raise ValueError('Production engine bytes changed')
        if any(sha(path) != report['sources'][name] for name, path in sources.items()):
            raise ValueError('Owner witness source changed')
        selected_name = selected['name']
        required = [row for row in report['results'] if row['engine'] == selected_name]
        baseline = [row for row in report['results'] if row['engine'] != selected_name]
        report['selected_contract_passed'] = len(required) == len(modes) and all(row['passed'] for row in required)
        report['native_baseline_failures'] = [dict(mode=row['mode'], exit=row['exit'],
                                                  stdout=row['stdout'], stderr=row['stderr'])
                                              for row in baseline if not row['passed']]
        # The new scopes and Rust owners exist only in the selected engine.
        # Native observations remain failed when they fail the same intended
        # lifecycle assertion; they are never excluded or counted as passed.
        report['native_passes_lost'] = [row['mode'] for row in baseline if row['passed'] and
                                       not next(other for other in required if other['mode'] == row['mode'])['passed']]
        report['passed'] = report['native_cpp_cleanup']['passed'] and report['selected_contract_passed'] and len(baseline) == len(modes) and not report['native_passes_lost']
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
