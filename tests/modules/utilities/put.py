#!/usr/bin/env python3
"""Complete pinned and hybrid-build Logger::put bodies with real shared-thread mutex, FILE writes and
stream publication."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated, write_report
from gate import PIN
from rust_asan import inspect as inspect_rust_asan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--threads', action='store_true', help='Required: the shared-thread fixture is the only supported profile')
    parser.add_argument('--sanitize', action='store_true')
    parser.add_argument('--rust-asan', action='store_true')
    parser.add_argument('--asan-runtime', type=Path, help='Explicit compatible dynamic ASan runtime; requires Rust ASan')
    parser.add_argument('--native-ubsan-only', action='store_true', help='Native UBSan with Rust ASan, for incompatible native/Rust ASan runtimes')
    args = parser.parse_args()
    if not args.threads:
        parser.error('The non-thread native-field model cannot exercise the owned Logger state; use --threads')
    if args.rust_asan and not args.sanitize:
        parser.error('--rust-asan requires --sanitize')
    if args.asan_runtime and not args.rust_asan:
        parser.error('--asan-runtime requires --rust-asan')
    if args.native_ubsan_only and not args.rust_asan:
        parser.error('--native-ubsan-only requires --rust-asan')
    fixture = Path(__file__).with_name('put_threads.cc')
    report = {'passed': False, 'pin': PIN, 'scope': __doc__,
              'native_sanitized': args.sanitize, 'rust_sanitized': args.rust_asan,
              'native_sanitizers': (['undefined'] if args.native_ubsan_only else ['address', 'undefined']) if args.sanitize else []}
    if args.rust_asan:
        report['rust_asan_objects'] = inspect_rust_asan(args.archive)
    with tempfile.TemporaryDirectory(prefix='swr-logger-put-') as temporary:
        tree = Path(temporary)
        original = subprocess.check_output(['git','-C',str(args.source),'show',PIN+':src/core/log.cc'], text=True)
        # The candidate is the hybrid build's log.cc; its Rust declarations come from that build.
        hybrid, flags = hybrid_tree(args.archive)
        report['hybrid_tree'] = str(hybrid)
        source = hybrid/'src/core/log.cc'
        rust_headers = [flag for flag in flags if flag.startswith('-I') and flag != '-I'+str(hybrid/'include')]
        oracle = ''
        owned_state = any(call in source.read_text() for call in ['swr_v1_utils_log_state_emit_step(', 'swr_v1_utils_log_path_emit_step('])
        report['owned_state'] = owned_state
        # localtime_r replaces localtime to exclude the original static-tm race.
        substitutions = {'std::localtime(': 'thread_localtime('}
        for name, content in [('reference',original),('candidate',source.read_text())]:
            body = content[content.index('void Logger::put('):content.index('\n}  // namespace swoole')]
            if name == 'candidate' and not any(call in body for call in ['swr_v1_utils_log_put_step(', 'swr_v1_utils_log_emit_step(', 'swr_v1_utils_log_state_emit_step(', 'swr_v1_utils_log_path_emit_step(']):
                raise ValueError('Selected Logger::put has no Rust controller; native fallback cannot qualify')
            for before, after in substitutions.items():
                body = body.replace(before, after)
            base = 'StateThreadFixture' if name == 'candidate' and owned_state else 'FixtureLogger'
            oracle += 'namespace '+name+' { struct Logger : '+base+' { void put(int, const char *, size_t); };\n'+body+'\n}\n'
        (tree/'put-oracle.inc').write_text(oracle)
        report['substitutions'] = substitutions
        binary = tree/'put'
        command = ['c++','-std=c++14','-O2','-Wall','-Wextra','-Werror','-UNDEBUG',
                   '-I'+str(tree),*rust_headers,str(fixture),
                   str(args.archive.resolve()),'-lpthread','-ldl','-lm','-o',str(binary)]
        if owned_state:
            command += ['-DM14_STATE_ABI2', '-I'+str(ROOT/'tests/modules/utilities')]
        if args.sanitize:
            command += ['-fsanitize=' + ('undefined' if args.native_ubsan_only else 'address,undefined'), '-fno-sanitize-recover=all']
        if args.asan_runtime:
            runtime = args.asan_runtime.resolve(strict=True)
            report['asan_runtime'] = str(runtime)
            command += ([] if args.native_ubsan_only else ['-fno-sanitize-link-runtime']) + [str(runtime), '-Wl,-rpath,' + str(runtime.parent)]
        result = run_isolated(command,timeout=90)
        report.update(compile_command=command,compile_exit=result.returncode,compile_stderr=result.stderr)
        if not result.returncode:
            result = run_isolated([str(binary)],timeout=90)
            report.update(exit=result.returncode,stdout=result.stdout,stderr=result.stderr,passed=result.returncode==0)
    write_report(args.output,report)
    print(json.dumps(report))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
