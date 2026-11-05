#!/usr/bin/env python3
"""Run a whole production C++ Server consumer against native/core-test SDKs.

Build unchanged core originals with tools/core_tests.py first. This consumer adds
runtime coverage for native bitfield layout, canonical aliases and counter/session
retirement; it never substitutes an extracted C++ method body.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shlex
import sys
import subprocess

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--core', type=Path, required=True, help='Completed core_tests.py --build-root')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--source', type=Path, help='Build original IPC tests and SDKs first')
    p.add_argument('--config', type=Path)
    p.add_argument('--archive', type=Path)
    p.add_argument('--jobs', type=int, default=2)
    a = p.parse_args()
    output = a.output.resolve()
    folder = output.with_suffix('')
    folder.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().with_suffix('.cc')
    report = {'scope': __doc__, 'consumer_sha256': sha(source), 'results': [], 'passed': False}
    try:
        if a.source:
            if not a.config or not a.archive:
                raise ValueError('--source requires --config and --archive')
            subprocess.run([sys.executable, str(ROOT / 'tools/core_tests.py'),
                            '--source', str(a.source), '--config', str(a.config),
                            '--archive', str(a.archive), '--build-root', str(a.core),
                            '--jobs', str(a.jobs), '--output', str(folder / 'originals.json'),
                            '--file', 'core-tests/src/os/pipe.cpp',
                            '--file', 'core-tests/src/os/msg_queue.cpp',
                            '--file', 'core-tests/src/os/process_pool.cpp'], check=True)
        for engine in ['upstream', 'hybrid']:
            build = a.core.resolve() / engine / 'build'
            report.setdefault('sdks', {})[engine] = {'flags_sha256': sha(build / 'CMakeFiles/core-tests.dir/flags.make'), 'link_sha256': sha(build / 'CMakeFiles/core-tests.dir/link.txt')}
            flags = {}
            for line in (build / 'CMakeFiles/core-tests.dir/flags.make').read_text().splitlines():
                if ' = ' in line:
                    key, value = line.split(' = ', 1)
                    flags[key] = shlex.split(value)
            link = shlex.split((build / 'CMakeFiles/core-tests.dir/link.txt').read_text())
            compiler = link[0]
            libraries = []
            skip = False
            for value in link[1:]:
                if skip:
                    skip = False
                elif value == '-o':
                    skip = True
                elif not value.endswith('.o') and not value.startswith('-std='):
                    libraries.append(value)
            for standard in ['c++14', 'c++17']:
                target = folder / (engine + '-' + standard)
                command = [compiler, *flags.get('CXX_DEFINES', []), *flags.get('CXX_INCLUDES', []),
                           *[v for v in flags.get('CXX_FLAGS', []) if not v.startswith('-std=')],
                           '-std=' + standard, '-UNDEBUG', str(source), '-o', str(target), *libraries]
                compiled = run_isolated(command, cwd=build, timeout=120)
                row = {'engine': engine, 'standard': standard, 'command': command, 'cwd': str(build),
                       'compile_exit': compiled.returncode, 'compile_stdout': compiled.stdout,
                       'compile_stderr': compiled.stderr}
                report['results'].append(row)
                if compiled.returncode:
                    write_report(output, report)
                    return 1
                result = run_isolated([str(target)], cwd=build, timeout=30)
                row.update(exit=result.returncode, stdout=result.stdout, stderr=result.stderr)
                write_report(output, report)
        report['passed'] = len(report['results']) == 4 and all(
            row.get('exit') == 0 and row.get('stdout') == 'worker-retirement-ok\n'
            for row in report['results'])
    except Exception as error:
        report['error'] = str(error)
    write_report(output, report)
    print(json.dumps({'passed': report['passed'], 'error': report.get('error')}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
