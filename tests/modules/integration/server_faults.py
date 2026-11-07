#!/usr/bin/env python3
"""Qualify ordinary C++ Server exception transport against the actual optimized archive."""
import argparse
import hashlib
import json
from pathlib import Path
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.harness import run_isolated, write_report


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--archive', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    archive = args.archive.resolve()
    source, flags = hybrid_tree(archive)
    header = archive.parents[1] / 'include/swoole_rs_corebase_native_exception.h'
    fixture = Path(__file__).with_suffix('.cc')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    wrapped = sys.platform.startswith('linux')
    report = dict(passed=False, scope=__doc__, hybrid_tree=str(source),
                  archive=str(archive), archive_sha256=sha(archive),
                  header_sha256=sha(header), fixture_sha256=sha(fixture),
                  ordinary_cpp_exceptions_only=True, rust_instrumented=False,
                  zend_bailout_qualified=False, custom_thread_exit_qualified=False,
                  pthread_forced_unwind_qualified=False,
                  rust_format_drop_attempted=wrapped, rust_format_drop_qualified=False, runs=[])
    with tempfile.TemporaryDirectory(prefix='server-faults-', dir=args.output.parent) as directory:
        for standard in ['c++14', 'c++17']:
            for sanitize in [False, True]:
                binary = Path(directory) / (standard + ('-asan' if sanitize else ''))
                command = ['c++', '-std=' + standard, '-O2', '-g', '-UNDEBUG',
                           '-Wall', '-Wextra', '-Werror', *flags, str(fixture),
                           str(archive), '-pthread', '-o', str(binary)]
                if wrapped:
                    # --gc-sections: the archive's references to C++ (event.cc's Reactor::wait) are unreachable here.
                    # OpenSSL: the server runner sends through crates/tls's Socket::send (crates/server/src/master_ops.rs).
                    command += ['-DWRAP_FREE', '-Wl,--wrap=free', '-Wl,--gc-sections', '-lssl', '-lcrypto', '-ldl', '-lm']
                if sanitize:
                    command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
                row = dict(standard=standard, native_sanitized=sanitize, commands=[])
                report['runs'].append(row)
                for argv in [command, [str(binary)]]:
                    result = run_isolated(argv, timeout=120)
                    row['commands'].append(dict(argv=argv, exit=result.returncode,
                                                stdout=result.stdout, stderr=result.stderr))
                    if result.returncode:
                        write_report(args.output, report)
                        print(json.dumps({'passed': False, 'output': str(args.output)}))
                        return 1
                if not result.stdout.startswith('native Server failure propagation: ') or not result.stdout.endswith(' trials PASS\n'):
                    raise ValueError('Native witness did not complete: ' + repr(result.stdout))
    report['rust_format_drop_qualified'] = wrapped
    report['passed'] = True
    write_report(args.output, report)
    print(json.dumps({'passed': True, 'runs': len(report['runs']),
                      'rust_format_drop_qualified': wrapped}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
