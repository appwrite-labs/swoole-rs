#!/usr/bin/env python3
"""Compare complete pinned and actual AsyncFile classes over traced real-file C ABI leaves."""
import argparse
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import native_flags
from tools.build_upstream import COMMIT
from tools.harness import run_isolated


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['source', 'archive', 'output']:
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    report = dict(passed=False, pin=COMMIT, commands=[], scope=__doc__,
                  native_sanitizers=True, rust_instrumented=False,
                  limitations=['C ABI leaves use traced real filesystem syscalls; actual coroutine backend is qualified separately by unchanged originals.',
                               'No physical allocation/OOM parity claim for removed private write-only storage.'])
    try:
        flags = native_flags(args.archive)
        with tempfile.TemporaryDirectory(prefix='async-file-', dir=args.output.parent) as directory:
            work = Path(directory)
            pinned = work / 'pin'
            data = subprocess.check_output(['git', '-C', str(args.source), 'archive', COMMIT, 'include', 'src/coroutine/file.cc'])
            with tarfile.open(fileobj=io.BytesIO(data)) as archive:
                for member in archive:
                    if member.isfile():
                        path = pinned / member.name
                        path.parent.mkdir(parents=True, exist_ok=True)
                        path.write_bytes(archive.extractfile(member).read())
            filesystem = work / 'fs'
            filesystem.mkdir()
            output = {}
            for engine in ['native', 'rust']:
                binary = work / engine
                includes = flags if engine == 'rust' else ['-I' + str(pinned / 'include')]
                command = ['c++', '-std=c++14', '-O1', '-g', '-Wall', '-Wextra', '-Werror',
                           '-ffunction-sections', '-fdata-sections', '-fsanitize=address,undefined',
                           '-fno-sanitize-recover=all', *includes,
                           *subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True).split(),
                           str(Path(__file__).with_suffix('.cc'))]
                command += [str(args.archive.resolve())] if engine == 'rust' else [str(pinned / 'src/coroutine/file.cc')]
                command += ['-Wl,-dead_strip'] if sys.platform == 'darwin' else ['-Wl,--gc-sections', '-ldl', '-lpthread', '-lm']
                command += ['-o', str(binary)]
                for command in [command, [str(binary), str(filesystem)]]:
                    result = run_isolated(command, timeout=90)
                    report['commands'].append(dict(engine=engine, argv=command, exit=result.returncode,
                                                   stdout=result.stdout, stderr=result.stderr))
                    if result.returncode:
                        raise RuntimeError(engine + ' AsyncFile caller failed')
                output[engine] = json.loads(result.stdout)
            assert output['native'] == output['rust'], 'AsyncFile effects/result/errno/copy-lifetime mismatch'
            report.update(passed=True, outcome=output['rust'])
    except Exception as error:
        report['error'] = str(error)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ['passed', 'scope']}))
    return not report['passed']


if __name__ == '__main__':
    raise SystemExit(main())
