#!/usr/bin/env python3
"""Check real Address SDK fields, value copies and shared per-thread scratch on both native cores."""
import argparse
import json
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[3]
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--core-build', type=Path, required=True)
p.add_argument('--native-report', type=Path, required=True)
p.add_argument('--output', type=Path, required=True)
a = p.parse_args()
a.output.parent.mkdir(parents=True, exist_ok=True)
native = json.loads(a.native_report.read_text())
if native.get('passed') is not True:
    raise ValueError('Require completed passing original Client/Address native cores')
report = {'passed': False, 'scope': __doc__, 'native_report': str(a.native_report), 'engines': []}
for engine in ['upstream', 'hybrid']:
    core = (a.core_build / engine).resolve()
    flags = (core / 'build/CMakeFiles/lib-swoole.dir/flags.make').read_text()
    compile_flags = [part for key in ['CXX_DEFINES', 'CXX_INCLUDES', 'CXX_FLAGS']
                     for part in shlex.split(next(line.split('=', 1)[1] for line in flags.splitlines() if line.startswith(key + ' =')))]
    link = shlex.split((core / 'build/CMakeFiles/core-tests.dir/link.txt').read_text())
    libraries = [arg for arg in link if (arg.startswith(('-L', '-l', '-Wl,-rpath,')) or arg.endswith(('.a', '.so', '.dylib'))) and 'gtest' not in arg and 'gmock' not in arg]
    binary = a.output.parent / ('address-record-' + engine)
    command = ['c++', '-std=c++14', '-UNDEBUG', *compile_flags, str(Path(__file__).with_suffix('.cc')), *libraries, '-pthread', '-o', str(binary)]
    build = subprocess.run(command, capture_output=True, text=True)
    row = {'engine': engine, 'command': command, 'compile_returncode': build.returncode, 'compile_stderr': build.stderr}
    if build.returncode == 0:
        run = subprocess.run([str(binary)], capture_output=True, text=True, timeout=30)
        row.update(returncode=run.returncode, stdout=run.stdout, stderr=run.stderr)
    report['engines'].append(row)
    if build.returncode or row.get('returncode'):
        break
report['passed'] = len(report['engines']) == 2 and all(row.get('returncode') == 0 for row in report['engines'])
a.output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'passed': report['passed']}))
raise SystemExit(int(not report['passed']))
