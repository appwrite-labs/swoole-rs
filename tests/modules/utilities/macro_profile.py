#!/usr/bin/env python3
"""Ordinary existing macro frontdoors through immutable production Logger DSOs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import uuid
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def definitions(text):
    lines = text.splitlines(keepends=True)
    result = {}
    i = 0
    while i < len(lines):
        match = re.match(r'#define\s+(swoole_\w+)\(', lines[i])
        if match:
            chunk = lines[i]
            while lines[i].rstrip().endswith('\\'):
                i += 1
                chunk += lines[i]
            result.setdefault(match[1], []).append(chunk)
        i += 1
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--production-receipt', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--candidate-header', type=Path)
    p.add_argument('--candidate-archive', type=Path)
    p.add_argument('--asan-runtime', type=Path)
    p.add_argument('--plain-candidate', action='store_true',
                   help='Linux ordinary PHP caller with an uninstrumented archive; standalone sanitizer controls remain separate')
    p.add_argument('--fixture', type=Path, default=Path(__file__).with_suffix('.cc'))
    p.add_argument('--wire', type=Path, default=Path(__file__).with_suffix('.php'))
    p.add_argument('--single-profile', action='store_true')
    p.add_argument('--extra-cxx-flag', action='append', default=[])
    p.add_argument('--cpp-standard', choices=['c++14','c++17','c++20'], default='c++17')
    args = p.parse_args()
    profiles = [(0, 0)] if args.single_profile else [(0, 0), (1, 0), (0, 1), (1, 1)]
    if bool(args.candidate_header) != bool(args.candidate_archive):
        p.error('Candidate header and archive must be supplied together')
    if args.plain_candidate and (sys.platform != 'linux' or not args.candidate_archive or args.asan_runtime):
        p.error('Plain candidate requires Linux and a candidate archive without ASan runtime')
    source = json.loads(args.production_receipt.read_text())
    if not source['passed']:
        raise ValueError('Require completed source/SDK-qualified production receipt')
    work = args.output.resolve().parent / ('macro-profile-' + uuid.uuid4().hex)
    work.mkdir()
    report = dict(passed=False, work=str(work), source_ref=source['source_attestation']['source_ref'],
                  scope=__doc__ + ' Caller macro branches only; actual SDK layout/configuration is unchanged. This is existing Native macro behavior, not Rust macro conversion or a rebuilt debug/trace world.',
                  fixture_path=str(args.fixture.resolve()), wire_path=str(args.wire.resolve()),
                  profiles=profiles, results=[])
    if args.candidate_archive:
        report['scope'] = ('Current private macro implementation linked from the actual utility Cargo archive, '
                           'with unchanged production Logger DSOs and exact layout SDKs. '
                           'Native macro control remains unchanged. Not a rebuilt production macro world.')
        report['candidate'] = dict(header=str(args.candidate_header), archive=str(args.candidate_archive),
                                   rust_asan=not args.plain_candidate and (bool(args.asan_runtime) or sys.platform != 'darwin'),
                                   ordinary_php_loader=args.plain_candidate,
                                   production_logger_instrumented=False)
    try:
        engine_rows = source['engines']
        engines = parse_engines([row['name'] + '=' + row['extension'] for row in engine_rows], 'php', [])
        includes = shlex.split(subprocess.check_output(['php-config', '--includes'], text=True))
        flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True))
        baseline = None
        for engine, identity in zip(engines, engine_rows):
            variant = 'selected' if identity['rust_feature_bits'] & 512 else 'native'
            sdk = Path(source['work']) / variant / 'include'
            expected = source['coherent_sdk'][variant]['headers_sha256']
            for name, digest in expected.items():
                if sha(Path(name)) != digest:
                    raise ValueError('SDK drift before macro compile: ' + name)
            if sha(Path(identity['extension'])) != identity['sha256']:
                raise ValueError('Production extension changed')
            if args.candidate_header and variant == 'selected':
                private_sdk = work / 'candidate-sdk'
                shutil.copytree(sdk, private_sdk)
                (private_sdk / 'swoole_log.h').write_bytes(args.candidate_header.read_bytes())
                (private_sdk / 'swoole_rs_utilities.h').write_bytes((ROOT / 'native/utilities/swoole_rs_utilities.h').read_bytes())
                sdk = private_sdk
            macros = definitions((sdk / 'swoole_log.h').read_text())
            if baseline is None:
                baseline = macros
            elif macros != baseline and not args.candidate_header:
                raise ValueError('Native and selected macro bodies differ: not this baseline profile')
            for debug, trace in profiles:
                folder = work / (variant + '-' + str(debug) + str(trace))
                folder.mkdir()
                config = '#define M14_MACRO_DEBUG ' + str(debug) + '\n#define M14_MACRO_TRACE ' + str(trace) + '\n'
                for name, variants in macros.items():
                    enabled = debug if name in ['swoole_debug', 'swoole_hex_dump'] else trace if name == 'swoole_trace_log' else 1
                    chunk = variants[0 if enabled else -1]
                    config += '#undef ' + name + '\n' + chunk
                (folder / 'macro-profile.h').write_text(config)
                helper = folder / 'm14_macro_profile.so'
                candidate = bool(args.candidate_archive and variant == 'selected')
                link = []
                if candidate:
                    if sys.platform == 'darwin':
                        if not args.asan_runtime:
                            raise ValueError('This private candidate requires the exact Rust ASan runtime')
                        link = [str(args.candidate_archive), str(args.asan_runtime),
                                '-Wl,-rpath,' + str(args.asan_runtime.parent),
                                '-Wl,-exported_symbol,_get_module']
                    else:
                        link = [str(args.candidate_archive), *([] if args.plain_candidate else ['-fsanitize=address,undefined']),
                                '-Wl,--exclude-libs,ALL', '-ldl', '-lpthread', '-lrt', '-lutil', '-lm']
                command = ['c++', '-std=' + args.cpp_standard, '-O2', '-Wall', '-Wextra', '-Werror', *args.extra_cxx_flag, '-DHAVE_CONFIG_H',
                           *(['-Wno-deprecated-declarations'] if sys.platform == 'darwin' else []),
                           *includes, '-I' + str(sdk), '-I' + str(folder), *flags,
                           *(['-bundle', '-undefined', 'dynamic_lookup'] if sys.platform == 'darwin' else ['-shared', '-fPIC']),
                           '-MD', '-MF', str(folder / 'profile.d'), str(args.fixture.resolve()), *link, '-o', str(helper)]
                if args.candidate_archive:
                    from tools.build_upstream import require_build_space
                    require_build_space(ROOT)
                compiled = run_isolated(command, timeout=90)
                row = dict(engine=engine.name, debug=debug, trace=trace,
                           candidate=candidate, command=command, compile_exit=compiled.returncode, compile_stdout=compiled.stdout,
                           compile_stderr=compiled.stderr, passed=False)
                report['results'].append(row)
                write_report(args.output, report)
                if compiled.returncode:
                    raise ValueError('Ordinary macro caller compilation failed')
                if candidate:
                    symbols = subprocess.check_output(['nm', str(helper)], text=True)
                    for export in ['swr_v2_utils_log_frontdoor_step', 'swr_v2_utils_log_hex_step']:
                        if not re.search(r'^[0-9a-fA-F]+\s+[Tt]\s+_?' + export + r'$', symbols, re.M):
                            raise ValueError('Missing defined private macro export: ' + export)
                    row['defined_candidate_exports'] = ['swr_v2_utils_log_frontdoor_step', 'swr_v2_utils_log_hex_step']
                runtime_env = {**os.environ, 'TZ': 'UTC0', 'LC_ALL': 'C'}
                if candidate and not args.plain_candidate:
                    runtime_env['ASAN_OPTIONS'] = 'detect_leaks=0'
                    if sys.platform == 'darwin':
                        runtime_env['DYLD_INSERT_LIBRARIES'] = str(args.asan_runtime)
                    else:
                        runtime_env['LD_PRELOAD'] = subprocess.check_output(['c++', '-print-file-name=libasan.so'], text=True).strip()
                with tempfile.TemporaryDirectory(prefix='m14-macro-positive-') as directory:
                    result = run_isolated(engine.command('-d', 'extension=' + str(helper), str(args.wire.resolve()), directory),
                                          timeout=30, env=runtime_env)
                    row.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                               extension_sha256_after=sha(Path(identity['extension'])),
                               files={p.name: dict(content=p.read_text()) for p in Path(directory).iterdir() if p.is_file()})
                write_report(args.output, report)
                if result.returncode or result.stderr or row['extension_sha256_after'] != identity['sha256']:
                    raise ValueError('Ordinary macro caller profile failed')
                row['summary'] = json.loads(result.stdout)
                row['passed'] = True
            for name, digest in expected.items():
                if sha(Path(name)) != digest:
                    raise ValueError('SDK drift after macro profiles')
        for debug, trace in profiles:
            summaries = [row['summary'] for row in report['results'] if row['debug'] == debug and row['trace'] == trace]
            if len(summaries) != 2 or summaries[0] != summaries[1]:
                raise ValueError('Native/selected ordinary macro summaries differ')
        report['passed'] = len(report['results']) == 2 * len(profiles)
    except Exception as error:
        report['error'] = type(error).__name__ + ': ' + str(error)
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
