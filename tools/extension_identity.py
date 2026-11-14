#!/usr/bin/env python3
"""Observe an existing reference extension; this receipt is not a build attestation."""
import argparse
from datetime import datetime, timezone
import hashlib
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_upstream import COMMIT
from tools.harness import Engine, write_report


def observe(engine, source):
    pin = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if pin != COMMIT:
        raise ValueError('Reference checkout differs from pinned source')
    before = hashlib.sha256(engine.extension.read_bytes()).hexdigest()
    metadata = engine.metadata()
    after = hashlib.sha256(engine.extension.read_bytes()).hexdigest()
    if before != after or metadata['sha256'] != before:
        raise ValueError('Reference extension changed during observation')
    if metadata['rust_feature_bits'] != 0:
        raise ValueError('Native reference unexpectedly advertises Rust features')
    return {'record_type': 'observed_reference_identity', 'build_attestation': False,
            'scope': 'Observed binary/PHP identity and separate pinned checkout; original build provenance is not established by this receipt.',
            'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_commit': pin,
            'extension_sha256': before, 'engine': metadata}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    write_report(args.output, observe(Engine('upstream', args.extension.resolve(strict=True), args.php,
                                             tuple(args.php_extension)), args.source))


if __name__ == '__main__':
    main()
