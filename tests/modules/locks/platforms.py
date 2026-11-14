#!/usr/bin/env python3
"""Compile the real host header and test synthetic unsupported-target rejection.

Synthetic preprocessing is a guard check, never a BSD/Android ABI/runtime pass.
"""
import argparse
import json
from pathlib import Path
import sys
import tempfile
import overlay

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import write_report, run_isolated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    overlay.SOURCE = args.source
    header = ROOT / 'native/locks/swoole_rs_locks.h'
    report = {'scope': __doc__, 'platform': sys.platform, 'upstream_pin': overlay.PIN,
              'results': [], 'passed': False}
    cases = [('host-cxx14', ''), ('host-c11', ''),
             ('synthetic-freebsd', '#undef __APPLE__\n#undef __linux__\n#define __FreeBSD__ 1\n'),
             ('synthetic-android', '#undef __APPLE__\n#define __linux__ 1\n#define __ANDROID__ 1\n')]
    if sys.platform == 'darwin':
        cases.append(('synthetic-apple-mobile', '#include <TargetConditionals.h>\n#undef TARGET_OS_OSX\n#define TARGET_OS_OSX 0\n'))
    with tempfile.TemporaryDirectory(prefix='swr-locks-platform-') as temporary:
        for name, defines in cases:
            source = Path(temporary) / (name + ('.c' if name == 'host-c11' else '.cc'))
            # Load host SDK integer declarations before simulating foreign macros.
            # This intentionally does not compile foreign pthread layouts.
            source.write_text('#include <stdint.h>\n#include <stddef.h>\n' + defines +
                              '#define SWOOLE_RS_LOCKS 1\n#include "swoole_rs_locks.h"\nint main(){return 0;}\n')
            command = ['cc' if name == 'host-c11' else 'c++', '-std=c11' if name == 'host-c11' else '-std=c++14', '-fsyntax-only', '-I' + str(header.parent), str(source)]
            result = run_isolated(command, timeout=30)
            expected = result.returncode == 0 if name.startswith('host-') else result.returncode != 0 and \
                'Rust locks require validated macOS or Linux platform contracts' in result.stderr
            report['results'].append({'name': name, 'source': source.read_text(), 'command': command,
                                      'exit': result.returncode, 'stderr': result.stderr,
                                      'expectation_met': expected})
    report['passed'] = all(row['expectation_met'] for row in report['results'])
    write_report(args.output, report)
    print(json.dumps({'passed': report['passed'], 'platform': sys.platform}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
