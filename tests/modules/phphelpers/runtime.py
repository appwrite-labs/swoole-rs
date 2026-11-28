#!/usr/bin/env python3
"""Actual PHP stdext copied arrays, native callbacks, autoload, object lifetime and bailout witnesses."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import sys

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
    parser.add_argument('--build-manifest', type=Path, required=True)
    args = parser.parse_args()
    fixture = Path(__file__).with_suffix('.php')
    bailout = Path(__file__).with_name('bailout.php')
    vm_bailout = Path(__file__).with_name('vm_bailout.php')
    report = dict(created_utc=datetime.now(timezone.utc).isoformat(), passed=False,
                  scope=__doc__, engines=[], results=[], bailout_results=[])
    try:
        engines = parse_engines(args.engine, args.php, args.php_extension)
        report['engines'] = [engine.metadata() for engine in engines]
        if {bool(engine['rust_feature_bits'] & 16777216) for engine in report['engines']} != {False, True}:
            raise ValueError('Require actual native and phphelpers-bit16777216 engines')
        selected = next(item for item in report['engines'] if item['rust_feature_bits'] & 16777216)
        report['source_attestation'] = attest(args.build_manifest, selected, 16777216)
        for engine, identity in zip(engines, report['engines']):
            result = run_isolated(engine.command(str(fixture)), timeout=30)
            row = dict(engine=engine.name, exit=result.returncode, stdout=result.stdout,
                       stderr=result.stderr, extension_sha256_after=sha(identity['extension']))
            report['results'].append(row)
            row['passed'] = result.returncode == 0 and not result.stderr and row['extension_sha256_after'] == identity['sha256']
            if row['passed']:
                try: row['summary'] = json.loads(result.stdout)
                except ValueError: row['passed'] = False
        if not all(row['passed'] for row in report['results']):
            raise ValueError('Actual PHP helper fixture failed; all engine first outputs retained')
        if len({json.dumps(row['summary'], sort_keys=True) for row in report['results']}) != 1:
            raise ValueError('Native/Rust helper summaries differ')
        # Each PHP process owns/reaps only its own new child. Native Zend fatal
        # diagnostics are expected only in the two ordinary exception cases.
        for engine, identity in zip(engines, report['engines']):
            for mode in ['direct','coroutine']:
                for outcome in ['return','exception']:
                    result = run_isolated(engine.command('-d','display_errors=stderr','-d','log_errors=0',str(bailout),mode,outcome),timeout=30)
                    row = dict(engine=engine.name, mode=mode, outcome=outcome, exit=result.returncode,
                               stdout=result.stdout, stderr=result.stderr,
                               extension_sha256_after=sha(identity['extension']), passed=False)
                    report['bailout_results'].append(row)
                    lines=result.stdout.splitlines()
                    try:
                        summary=json.loads(lines[-1])
                        expected=dict(coroutine=mode=='coroutine',exception=outcome=='exception',
                                      code=255 if outcome=='exception' else 0,signal=0)
                        markers=['M26 callback enter']+([] if outcome=='exception' else ['M26 callback return'])
                        diagnostic=(('RuntimeException: M26 ordinary child exception' in result.stderr and 'thrown in' in result.stderr)
                                    if outcome=='exception' else not result.stderr)
                        row['summary']=summary
                        row['passed']=(result.returncode==0 and summary==expected and lines[:-1]==markers and diagnostic
                                       and row['extension_sha256_after']==identity['sha256'])
                    except (ValueError,IndexError): pass
        if not all(row['passed'] for row in report['bailout_results']):
            raise ValueError('Isolated native Zend callback/bailout witness failed; all first outputs retained')
        # exit() from a PCRE/JSON warning handler or a VM destructor inside a stdext call.
        report['vm_bailout_results'] = []
        for engine, identity in zip(engines, report['engines']):
            for mode, expected in [('pcre-exit', 37), ('vm-exit', 38), ('json-exit', 39)]:
                result = run_isolated(engine.command(str(vm_bailout), mode), timeout=30)
                row = dict(engine=engine.name, mode=mode, exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                           extension_sha256_after=sha(identity['extension']))
                row['passed'] = (result.returncode == expected and result.stdout == 'before\nexit\n' and not result.stderr
                                 and row['extension_sha256_after'] == identity['sha256'])
                report['vm_bailout_results'].append(row)
        if not all(row['passed'] for row in report['vm_bailout_results']):
            raise ValueError('Actual VM/PCRE/JSON bailout witness failed; original outputs retained')
        unchanged(args.build_manifest, report['source_attestation'])
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
