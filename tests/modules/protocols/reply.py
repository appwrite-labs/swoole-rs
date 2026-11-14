#!/usr/bin/env python3
"""Compare actual PHP Redis formatting and Zend callback/error order."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='First engine is the reference')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='PHP module loaded before Swoole')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/protocols-reply.json')
    args = parser.parse_args()
    script = Path(__file__).with_suffix('.php')
    oom_script = script.with_name('reply_oom.php')
    report = {'created_utc': datetime.now(timezone.utc).isoformat(),
              'scope': 'Actual Zend Redis formatting, nested lists/maps, binary/empty replies, scalar conversions, exception and callback order. Invalid non-array SET/MAP is excluded because original dereferences after throwing.',
              'engines': [], 'results': []}
    reference = None
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        run = subprocess.run(engine.command(str(script)), capture_output=True, text=True, timeout=30)
        row = {'engine': engine.name, 'returncode': run.returncode, 'stderr': run.stderr, 'passed': False}
        try:
            assert run.returncode == 0 and not run.stderr, 'PHP execution failed'
            cases = json.loads(run.stdout)
            assert len(cases) == 121, 'Incomplete corpus'
            assert len({case['name'] for case in cases}) == len(cases), 'Duplicate cases'
            if reference is None:
                reference = cases
            row['cases'] = cases
            row['mismatches'] = [case['name'] for expected, case in zip(reference, cases) if expected != case]
            oom = subprocess.run(engine.command(str(oom_script)), capture_output=True, text=True, timeout=10)
            assert oom.stdout, 'Missing payload-ready checkpoint before allocator bailout'
            checkpoint = json.loads(oom.stdout.splitlines()[0])
            assert checkpoint['checkpoint'] == 'payload-ready' and checkpoint['payload_bytes'] == 8388608
            assert checkpoint['payload_sha256'] == hashlib.sha256(b'x' * 8388608).hexdigest()
            exhausted = oom.returncode == 255 and 'Allowed memory size' in oom.stderr and 'exhausted' in oom.stderr
            assert exhausted and 'panicked at' not in oom.stderr, 'Expected isolated PHP allocation bailout'
            row['allocator_bailout'] = {'checkpoint': checkpoint, 'returncode': oom.returncode, 'stderr': oom.stderr}
            row['passed'] = not row['mismatches']
        except (AssertionError, ValueError) as error:
            row['error'] = str(error)
            row['stdout'] = run.stdout
        report['results'].append(row)
        write_report(args.output, report)
    print(json.dumps({'passed': sum(row['passed'] for row in report['results']), 'engines': len(report['results'])}))
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
