#!/usr/bin/env python3
"""Actual PHP Logger binding, file reconfiguration, rotation, filtering and fork checks."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import parse_engines, run_isolated, write_report
from tools.ext_identity import attest, unchanged


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--build-manifest', type=Path)
    args = parser.parse_args()
    fixture = Path(__file__).with_suffix('.php')
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), passed=False,
                  scope=__doc__, engines=[], results=[])
    try:
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(engine['rust_feature_bits'] & 512) for engine in report['engines']} != {False, True}:
            raise ValueError('Require actual native and utilities-bit512 engines')
        report['source_attestation'] = None
        if args.build_manifest:
            selected = next(item for item in report['engines'] if item['rust_feature_bits'] & 512)
            report['source_attestation'] = attest(args.build_manifest, selected, 512)
        for engine, identity in zip(engines, report['engines']):
            with tempfile.TemporaryDirectory(prefix='swr-php-logger-') as directory:
                result = run_isolated(engine.command(str(fixture), directory), timeout=45)
                row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout,
                           stderr=result.stderr, files={path.name: dict(size=path.stat().st_size,
                           content=path.read_text()) for path in sorted(Path(directory).iterdir()) if path.is_file()},
                           extension_sha256_after=sha(identity['extension']))
                report['results'].append(row)
                if row['extension_sha256_after'] != identity['sha256']:
                    raise ValueError('Engine module changed during check')
                if result.returncode or result.stderr:
                    raise ValueError('PHP Logger fixture failed; raw outputs retained')
                row['summary'] = json.loads(result.stdout)
        if len({json.dumps(row['summary'], sort_keys=True) for row in report['results']}) != 1:
            raise ValueError('Native/Rust Logger summaries differ')
        if args.build_manifest:
            unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
