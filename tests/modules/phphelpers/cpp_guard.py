#!/usr/bin/env python3
"""Require captured C++ custody to survive a native Zend catch without PHP publication."""
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
from tools.hybrid_identity import attest


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--php', required=True)
    parser.add_argument('--php-config', required=True)
    parser.add_argument('--engine', required=True)
    parser.add_argument('--export', type=Path, required=True)
    parser.add_argument('--source-ref', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    work = output.parent / (output.stem + '-files')
    work.mkdir(parents=True, exist_ok=False)
    report = dict(passed=False, results=[], scope=__doc__, work=str(work))
    try:
        engine, = parse_engines([args.engine], args.php)
        identity = engine.metadata()
        if not identity['rust_feature_bits']:
            raise ValueError('Require an actual selected production engine')
        report['engine'] = identity
        report['attestation'] = attest(args.export / 'build-manifest.json', args.export / 'libswoole_rs_ffi.a',
                                       identity, args.source_ref, 0)
        receipt = json.loads((args.export / 'export.json').read_text())
        manifest = json.loads((args.export / 'build-manifest.json').read_text())
        prefix = Path(receipt['files']['libswoole_rs_ffi.a']['source']).parents[1]
        header = prefix / 'include/swoole_rs_zend_guard.h'
        if sha(header) != manifest['inputs']['headers_sha256']['native/corebase/swoole_rs_zend_guard.h']:
            raise ValueError('Actual SDK guard header differs from build manifest')
        (work / header.name).write_bytes(header.read_bytes())
        report['header_sha256'] = sha(header)
        scope_header = prefix / 'include/swoole_rs_native_cpp_scope.h'
        if sha(scope_header) != manifest['inputs']['headers_sha256']['native/corebase/swoole_rs_native_cpp_scope.h']:
            raise ValueError('Actual native C++ scope header differs from build manifest')
        (work / scope_header.name).write_bytes(scope_header.read_bytes())
        report['scope_header_sha256'] = sha(scope_header)
        provider_header = prefix / 'include/swoole_rs_native_effect.h'
        if sha(provider_header) != manifest['inputs']['headers_sha256']['native/corebase/swoole_rs_native_effect.h']:
            raise ValueError('Actual optional provider header differs from build manifest')
        (work / provider_header.name).write_bytes(provider_header.read_bytes())
        report['provider_header_sha256'] = sha(provider_header)
        sources = [Path(__file__).with_suffix('.cc'), Path(__file__).with_suffix('.php'),
                   Path(__file__).with_name('exit_guard.rs'), Path(__file__).with_name('cpp_scope_sdk.cc'),
                   Path(__file__).with_name('cpp_scope_sdk_other.cc')]
        for source in sources:
            (work / source.name).write_bytes(source.read_bytes())
        report['sources'] = {str(source): sha(source) for source in sources}
        archive = work / 'libcpp_guard_owner.a'
        command = ['rustc', '--edition=2024', '--crate-type=staticlib', '-Copt-level=3', '-Cpanic=abort',
                   str(work / 'exit_guard.rs'), '-o', str(archive)]
        result = run_isolated(command, timeout=120)
        report['rust_compile'] = dict(command=command, exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
        write_report(output, report)
        if result.returncode:
            raise ValueError('Rust owner witness did not compile')
        includes = shlex.split(subprocess.check_output([args.php_config, '--includes'], text=True))
        for standard in ['c++14', 'c++17']:
            sdk = work / ('cpp-sdk-' + standard)
            command = ['c++', '-std=' + standard, '-O2', '-I' + str(work),
                       str(work / 'cpp_scope_sdk.cc'), str(work / 'cpp_scope_sdk_other.cc'), '-o', str(sdk)]
            compiled = run_isolated(command, timeout=120)
            sdk_row = dict(command=command, exit=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr)
            if compiled.returncode:
                report['results'].append(dict(standard=standard, sdk=sdk_row, passed=False))
                write_report(output, report)
                continue
            executed = run_isolated([str(sdk)], timeout=30)
            undefined = subprocess.check_output(['nm', '-u', str(sdk)], text=True)
            sdk_row.update(runtime_exit=executed.returncode, runtime_stdout=executed.stdout,
                           runtime_stderr=executed.stderr, undefined_symbols=undefined,
                           passed=executed.returncode == 0 and not executed.stdout and not executed.stderr and 'zend_' not in undefined)
            helper = work / ('cpp-guard-' + standard + '.so')
            command = ['c++', '-std=' + standard, '-O2', '-shared', '-fPIC',
                       '-DZEND_ENABLE_STATIC_TSRMLS_CACHE=1', *includes, '-I' + str(work),
                       str(work / 'cpp_guard.cc'), str(archive), '-lpthread', '-ldl', '-lm', '-o', str(helper)]
            result = run_isolated(command, timeout=120)
            row = dict(standard=standard, sdk=sdk_row, compile=dict(command=command, exit=result.returncode,
                                                       stdout=result.stdout, stderr=result.stderr), passed=False)
            report['results'].append(row)
            write_report(output, report)
            if result.returncode:
                continue
            helper_sha = sha(helper)
            row['runtimes'] = []
            for mode in ['plain', 'catch', 'coroutine', 'coroutine-catch', 'non-lifo']:
                command = engine.command('-d', 'extension=' + str(helper), work / 'cpp_guard.php', mode)
                result = run_isolated(command, timeout=30)
                row['runtimes'].append(dict(mode=mode, command=list(map(str, command)), exit=result.returncode,
                                            stdout=result.stdout, stderr=result.stderr,
                                            passed=result.returncode == 0 and result.stdout == 'cpp-chain: ok\n' and not result.stderr))
                write_report(output, report)
            row['passed'] = sdk_row['passed'] and all(run['passed'] for run in row['runtimes'])
            if sha(helper) != helper_sha:
                raise ValueError('Native witness bytes changed')
            write_report(output, report)
        if sha(engine.extension) != identity['sha256']:
            raise ValueError('Production engine bytes changed')
        if any(sha(source) != report['sources'][str(source)] for source in sources):
            raise ValueError('Witness source changed')
        report['passed'] = len(report['results']) == 2 and all(row['passed'] for row in report['results'])
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
