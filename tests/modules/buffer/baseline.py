#!/usr/bin/env python3
"""Diagnose pinned upstream buffer behavior without changing its source/tests."""
import argparse
from datetime import datetime, timezone
import io
import json
import os
from pathlib import Path
import platform
import shlex
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.harness import run_compiler

from tools.build_upstream import COMMIT as PIN
FILES = ['src/core/buffer.cc', 'src/core/string.cc', 'include/swoole_buffer.h',
         'core-tests/src/memory/buffer.cpp']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/buffer-baseline.json'))
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    local = Path(__file__).resolve().parent
    report = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
              'pin': PIN, 'host': platform.platform(), 'sanitized': args.sanitize,
              'adapter': 'Original buffer.cpp assertions mapped to throwing ASSERT_EQ; TEST names one callable; deterministic xorshift random helper; startup installs libc allocator for actual String helper. Actual upstream buffer.cc, string.cc and headers compiled unchanged.',
}
    with tempfile.TemporaryDirectory(prefix='swr-buffer-baseline-') as temporary:
        tree = Path(temporary)
        archive = subprocess.check_output(['git', '-C', str(args.source), 'archive', PIN,
                                           'include', *[path for path in FILES if not path.startswith('include/')]])
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            for member in bundle:
                if member.isfile():
                    destination = tree / member.name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(bundle.extractfile(member).read())
        for path in FILES:
            # Upstream marks core-tests export-ignore; read that exact Git blob.
            if not (tree / path).is_file():
                (tree / path).parent.mkdir(parents=True, exist_ok=True)
                (tree / path).write_bytes(subprocess.check_output([
                    'git', '-C', str(args.source), 'show', f'{PIN}:{path}']))
        binary = tree / 'baseline'
        command = shlex.split(os.environ.get('CXX', 'c++')) + [
            '-std=c++17', '-O1', '-g', '-ffunction-sections', '-fdata-sections',
            '-I' + str(local), '-I' + str(tree / 'include'),
            str(tree / 'src/core/buffer.cc'), str(tree / 'src/core/string.cc'),
            str(tree / 'core-tests/src/memory/buffer.cpp'), str(local / 'baseline.cc'),
            '-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections',
        ]
        if args.sanitize:
            command.append('-fsanitize=address,undefined')
        command += ['-o', str(binary)]
        report['compile_command'] = command
        compiled = run_compiler(command, timeout=45)
        report['compile'] = compiled
        if compiled['returncode']:
            write_report(args.output, report)
            print(compiled['stderr'], file=sys.stderr)
            return 1
        result = run_isolated([str(binary)], timeout=15)
        report['baseline'] = {'returncode': result.returncode, 'stderr': result.stderr}
        if result.returncode:
            write_report(args.output, report)
            print(result.stderr, file=sys.stderr)
            return 1
        report['baseline']['outcome'] = json.loads(result.stdout)
        report['excluded_inputs'] = 'Empty vectors and offsets >= total are outside this diagnostic: upstream lacks guards and can loop or access outside vector bounds. They are not counted as passing cases; proposed Rust contract must define them explicitly.'
        report['interpretation'] = 'Diagnostic success means pinned behavior reproduced, NOT that buffer accounting or copied bytes are correct. Mismatches are recorded as baseline defects, not passing compatibility cases.'
        write_report(args.output, report)
        outcome = report['baseline']['outcome']
        print(f'Original assertion runs={outcome["original_assertion_runs"]}; accounting mismatch cases={outcome["accounting_mismatch_cases"]}; minimal underflow={outcome["minimal_underflow"]}; report={args.output}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
