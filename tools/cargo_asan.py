#!/usr/bin/env python3
"""A crate's unit tests under Rust ASan (coroutine-rust-asan, memory-rust-asan): they handle raw
pointers. The plain run is the crate's ordinary cargo test."""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated, write_report  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--package', required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    host = next(line.split()[1] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines()
                if line.startswith('host:'))
    # --target keeps build scripts and proc macros uninstrumented; its own target
    # directory keeps the ordinary build cache intact.
    command = ['cargo', 'test', '--locked', '-p', args.package, '--target', host,
               '--target-dir', str(ROOT / 'target/asan')]
    env = {**os.environ, 'RUSTC_BOOTSTRAP': '1', 'RUSTFLAGS': '-Zsanitizer=address',
           'RUSTDOCFLAGS': '-Zsanitizer=address'}
    result = run_isolated(command, cwd=ROOT, env=env, timeout=900)
    report = dict(command=command, exit=result.returncode, stdout=result.stdout[-20000:],
                  stderr=result.stderr[-20000:], passed=result.returncode == 0)
    write_report(args.report.resolve(), report)
    print(json.dumps({'passed': report['passed']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
