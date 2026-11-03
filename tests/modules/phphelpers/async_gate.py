#!/usr/bin/env python3
"""Whole common async helper with ordinary Native recorders, not runtime/DB proof."""
import argparse
import json
import subprocess
import sys
import uuid
from pathlib import Path
from gate import ROOT, PIN, stage, body
sys.path.insert(0, str(ROOT))
from tools.build_upstream import require_build_space
from tools.harness import run_isolated, write_report
DEST = 'ext-src/php_swoole_cxx.h'
SIGNATURE = 'static inline bool php_swoole_async(bool blocking, const std::function<void(void)> &fn) {'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--archive', type=Path, help='Source-matched selected FFI archive; query real ABI1/bit16777216')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists():
        parser.error('Use a unique receipt')
    work = output.parent / ('phphelpers-async-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, scope=__doc__, work=str(work),
                  commands=[], results=[], measurement_performed=False, native_runtime_executed=False)

    def run(command):
        require_build_space(work)
        result = run_isolated(list(map(str, command)), cwd=work, timeout=120)
        report['commands'].append(dict(command=list(map(str, command)), exit=result.returncode,
                                       stdout=result.stdout, stderr=result.stderr))
        write_report(output, report)
        if result.returncode or result.stderr:
            raise ValueError('Required ordinary control failed or warned')
        return result

    try:
        report['sources'] = stage(args.source, work / 'current')
        original = subprocess.check_output(['git', '-C', str(args.source), 'show', PIN + ':' + DEST])
        new_body = body((work / 'current' / DEST).read_text(), SIGNATURE)
        # Exact original body is retained, apart from guard-adjacent newlines.
        reference = body(original.decode(), SIGNATURE)
        inner = reference[reference.index('{') + 1:-1].strip('\n')
        if '\n#else\n' + inner + '\n#endif\n' not in new_body:
            raise ValueError('Native async fallback changed')
        (work / 'async-reference.inc').write_text(reference)
        (work / 'async-selected.inc').write_text(new_body)
        archive = args.archive.resolve() if args.archive else work / 'libphphelpers.a'
        if not args.archive:
            run(['rustc', '--edition=2024', '--crate-type=staticlib', '-C', 'opt-level=2', '-D', 'warnings', ROOT / 'crates/phphelpers/src/lib.rs', '-o', archive])
        abi = work / 'async-abi.o'
        c_command = ['cc', '-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', '-UNDEBUG',
                     '-I' + str(ROOT / 'native/phphelpers'), '-I' + str(ROOT / 'native/include'),
                     '-c', Path(__file__).with_name('async_abi.c'), '-o', abi]
        if args.archive:
            c_command += ['-DSWR_PHPHELPERS_FFI']
        run(c_command)
        link = ['c++', abi, archive, '-lpthread', '-lm', '-o', work / 'async-abi']
        if sys.platform == 'linux':
            link += ['-ldl']
        run(link)
        run([work / 'async-abi'])
        for standard in ['c++14', 'c++17']:
            for sanitize in [False, True]:
                binary = work / (standard + '-' + str(sanitize))
                command = ['c++', '-std=' + standard, '-O2', '-Wall', '-Wextra', '-Werror', '-UNDEBUG',
                           '-DSWOOLE_RS_PHPHELPERS', '-I' + str(work), '-I' + str(ROOT / 'native/phphelpers'),
                           Path(__file__).with_name('async_ordinary.cc'), archive, '-lpthread', '-lm', '-o', binary]
                if sys.platform == 'linux':
                    command += ['-ldl']
                if sanitize:
                    command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
                run(command)
                result = json.loads(run([binary]).stdout)
                if result != dict(ordinary_dispatch_cases=8, native_runtime_executed=False):
                    raise ValueError('Incomplete ordinary dispatch output')
                report['results'].append(dict(standard=standard, native_cpp_sanitized=sanitize,
                                               rust_instrumented=False, result=result))
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
