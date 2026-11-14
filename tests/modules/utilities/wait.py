#!/usr/bin/env python3
"""Pinned misc.cc futex wait/wake bodies versus the Rust kernel and shared forks (Linux only)."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report
from tools.build_upstream import COMMIT as PIN


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--output', default=str(ROOT/'target/reports/utilities-wait.json'))
    p.add_argument('--sanitize', action='store_true')
    args = p.parse_args()
    if not sys.platform.startswith('linux'):
        p.error('Rust implements only the futex path; other platforms poll the Rust wait registry')
    archive = args.archive.resolve(strict=True)
    original = subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:src/core/misc.cc'], text=True)
    caller = Path(__file__).with_suffix('.cc')
    report = {'pin': PIN,
              'scope': 'Defined LP64 original misc.cc wait/wake result, errno, word and actual process-shared wake; no performance claim. C++ sanitizer only; Rust uninstrumented.',
              'passed': False}
    with tempfile.TemporaryDirectory(prefix='swr-utils-wait-') as temp:
        tree = Path(temp)
        oracle = original[original.index('#ifdef HAVE_FUTEX'):original.index('/* {{{ DJBX33A')]
        oracle = oracle.replace('#include <linux/futex.h>\n', '').replace('#include <sys/syscall.h>\n', '')
        (tree/'oracle.inc').write_text(oracle)
        binary = tree/'wait'
        command = ['c++', '-std=c++14', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-ignored-qualifiers',
                   '-I'+str(tree), '-I'+str(ROOT/'native/utilities'), str(caller), str(archive), '-o', str(binary)]
        if sys.platform.startswith('linux'):
            command += ['-ldl', '-lpthread', '-lrt', '-lutil', '-lm']
        if args.sanitize:
            command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
        compile_result = run_isolated(command, timeout=90)
        report.update(compile=command, compile_exit=compile_result.returncode, compile_stderr=compile_result.stderr)
        if compile_result.returncode:
            write_report(args.output, report)
            print(compile_result.stderr)
            return 1
        result = run_isolated([str(binary)], timeout=30)
        report.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode == 0:
            report['observations'] = json.loads(result.stdout)
            report['passed'] = True
    write_report(args.output, report)
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
