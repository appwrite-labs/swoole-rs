#!/usr/bin/env python3
"""Attested collection facade behavior on copied immutable PHP artifacts."""
import argparse
import json
from pathlib import Path
import shutil
import sys
import uuid
import hashlib
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


from tools.harness import Engine, parse_engines, run_isolated, write_report
from tools.ext_identity import attest, unchanged
from tools.build_upstream import require_build_space

BIT = 33554432
EXPECTED_CHECKS = 330
FIXTURE = Path(__file__).with_name('php_collections.php')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=['complete', 'column-bytes'], default='complete',
                        help='Independent named assertion inventory; column-bytes supplements complete')
    parser.add_argument('--engine', action='append', required=True, help='NAME=DSO; a native and a selected engine')
    parser.add_argument('--build-manifest', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    fixture, expected_checks = (FIXTURE, EXPECTED_CHECKS) if args.profile == 'complete' else (
        Path(__file__).with_name('php_column_bytes.php'), 16)
    output = args.output.resolve()
    work = output.parent / ('phpcollections-runtime-' + uuid.uuid4().hex)
    work.mkdir(parents=True)
    report = dict(passed=False, scope=__doc__, work=str(work), expected_checks=expected_checks,
                  profile=args.profile, engines=[], results=[], metadata_commands=[])
    try:
        engines = parse_engines(args.engine, args.php, args.php_extension)
        copies = []
        for index, engine in enumerate(engines):
            require_build_space(ROOT)
            require_build_space(work)
            expected = sha(engine.extension)
            copied = work / f'engine-{index}.so'
            shutil.copy2(engine.extension, copied)
            if sha(copied) != expected:
                raise ValueError('Extension copy identity differs')
            copies.append(Engine(engine.name, copied, engine.php, engine.php_extensions))
        probe = r'''if (!extension_loaded('swoole')) { fwrite(STDERR,'swoole missing'); exit(1); }
        echo json_encode(['php'=>PHP_VERSION,'swoole'=>phpversion('swoole'),'zts'=>PHP_ZTS,
        'rust_feature_bits'=>defined('SWOOLE_RS_FEATURE_BITS')?SWOOLE_RS_FEATURE_BITS:0]);'''
        for engine in copies:
            result = run_isolated(engine.command('-r', probe), timeout=10, cwd=work)
            report['metadata_commands'].append(dict(engine=engine.name, command=engine.command('-r', probe),
                                                     exit=result.returncode, stdout=result.stdout, stderr=result.stderr))
            if result.returncode or result.stderr:
                raise ValueError('Extension metadata probe failed')
            identity = json.loads(result.stdout)
            identity.update(name=engine.name, extension=str(engine.extension), sha256=sha(engine.extension),
                            php_extensions=list(engine.php_extensions))
            report['engines'].append(identity)
        selected = [identity for identity in report['engines'] if identity['rust_feature_bits'] & BIT]
        if len(selected) != 1 or not any(not identity['rust_feature_bits'] & BIT for identity in report['engines']):
            raise ValueError('Require actual native and M27-bit33554432 engines')
        report['source_attestation'] = attest(args.build_manifest, selected[0], BIT)
        for engine, identity in zip(copies, report['engines']):
            result = run_isolated(engine.command(fixture), timeout=30, cwd=work)
            row = dict(engine=engine.name, command=engine.command(fixture), exit=result.returncode,
                       stdout=result.stdout, stderr=result.stderr, extension_sha256_after=sha(engine.extension))
            report['results'].append(row)
            row['passed'] = result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']
            if row['passed']:
                try:
                    row['summary'] = json.loads(result.stdout)
                    if not isinstance(row['summary'], dict) or row['summary'].get('checks') != expected_checks:
                        row.update(passed=False, inventory_error='Collection assertion inventory differs from expected' + str(expected_checks))
                except ValueError as error:
                    row.update(passed=False, parse_error=str(error))
        if not all(row['passed'] for row in report['results']):
            raise ValueError('Collection runtime failed; all first outputs retained')
        if len({json.dumps(row['summary'], sort_keys=True) for row in report['results']}) != 1:
            raise ValueError('Native/selected collection summaries differ')
        unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report.update(passed=False, error=f'{type(error).__name__}: {error}')
    write_report(output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error', 'work']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
