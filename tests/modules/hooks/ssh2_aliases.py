#!/usr/bin/env python3
"""Compare 18 pinned SSH2 alias argument policies with Rust using actual SDK declarations.

The SDK calls return fixed local observations; this does not qualify network I/O.
Requires an SSH2-enabled Zend-free archive and the installed libssh2 SDK.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT
from tools.harness import run_isolated


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--archive', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    a.output = a.output.resolve()
    a.output.parent.mkdir(parents=True, exist_ok=True)
    original = subprocess.check_output(['git', '-C', str(a.source), 'show',
                                       f'{COMMIT}:ext-src/php_swoole_ssh2_hook.h'], text=True)
    generated = (ROOT / 'native/hooks/php_swoole_ssh2_hook.h').read_text()
    names = lambda text: set(re.findall(r'^#define (libssh2_\w+)\(', text, re.M))
    if names(original) != names(generated):
        raise ValueError('The generated dispatch changed the pinned interposition set')
    flags = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'libssh2'], text=True))
    report = dict(pin=COMMIT, scope=__doc__, passed=False, dispatch_count=len(names(original)), results=[],
                  archive=str(a.archive.resolve()), archive_sha256=hashlib.sha256(a.archive.read_bytes()).hexdigest())
    with tempfile.TemporaryDirectory(prefix='ssh2-aliases-', dir=a.output.parent) as directory:
        work = Path(directory)
        (work / 'php_swoole_ssh2_hook.h').write_text(original)
        for name in ['upstream', 'hybrid']:
            binary = work / name
            command = ['c++', '-std=c++14', '-O1', '-UNDEBUG', '-I' + str(work if name == 'upstream' else ROOT / 'native/hooks'),
                       str(ROOT / 'tests/modules/hooks/ssh2_aliases.cpp')]
            if name == 'hybrid':
                command += ['-DSWR_HYBRID', str(a.archive.resolve()), '-lpthread']
                if sys.platform.startswith('linux'):
                    command += ['-ldl', '-lm']
            command += [*flags, '-o', str(binary)]
            build = run_isolated(command, timeout=60)
            run = run_isolated([str(binary)], timeout=10) if build.returncode == 0 else build
            report['results'].append(dict(engine=name, command=command, compile_exit=build.returncode,
                                          compile_stderr=build.stderr, exit=run.returncode, stdout=run.stdout, stderr=run.stderr))
            a.output.write_text(json.dumps(report, indent=2) + '\n')
            if run.returncode or run.stdout != 'aliases=18\n' or run.stderr:
                return 1
        report['passed'] = True
        a.output.write_text(json.dumps(report, indent=2) + '\n')
    print('18 aliases and all pinned dispatch names match the SDK on both engines')
    return 0


if __name__ == '__main__':
    sys.exit(main())
