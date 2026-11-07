#!/usr/bin/env python3
"""Run a core differential against the exact verified hybrid archive."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT
from tools.build_hybrid import MODULES


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--module', choices=['error'], required=True)
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    manifest = json.loads(args.build_manifest.read_text())
    if args.module not in manifest['inputs']['modules'] or not manifest['migrated_feature_bits'] & MODULES[args.module][1]:
        parser.error('Selected build does not enable this module')
    prefix = next(value.split('=', 1)[1] for value in manifest['configure'] if value.startswith('--with-swoole-rs='))
    archive = (Path(prefix) / 'lib/libswoole_rs_ffi.a').resolve(strict=True)
    # The differential reads its patched sources and headers from this archive's build.
    if sha(archive) != manifest['inputs']['rust_archive_sha256']:
        parser.error('Archive differs from selected build')
    return subprocess.run([sys.executable, str(ROOT / 'tests/modules/error/differential.py'),
                           '--source', str(ROOT / 'target/upstream/src'), '--archive', str(archive),
                           '--output', str(args.output), *(['--sanitize'] if args.sanitize else [])], cwd=ROOT).returncode


if __name__ == '__main__':
    sys.exit(main())
