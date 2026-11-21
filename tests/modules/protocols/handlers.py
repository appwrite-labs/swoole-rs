#!/usr/bin/env python3
"""Compare actual Zend Redis handler registration/lookup and fatal validation."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, run_isolated, write_report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='PHP module loaded before Swoole')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/protocols-handlers.json')
    args = parser.parse_args()
    script = Path(__file__).with_suffix('.php')
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'engines': [], 'results': []}
    reference = None
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        row = {'engine': engine.name, 'passed': False}
        try:
            run = run_isolated(engine.command(script), timeout=10)
            assert run.returncode == 0 and not run.stderr, run.stderr + run.stdout
            value = json.loads(run.stdout)
            assert len(value['lookups']) == 529
            assert len(value['properties']) == 7
            if reference is None:
                reference = value
            assert value == reference, 'Actual registration/lookup/property/error order differs'
            row['outcome'] = value
            row['invalid_registration'] = []
            for length in [0, 64, 1024]:
                invalid = run_isolated(engine.command(script, length), timeout=10)
                assert invalid.returncode == 255 and 'invalid command' in invalid.stdout + invalid.stderr
                assert 'unexpected-valid' not in invalid.stdout
                row['invalid_registration'].append({'bytes': length, 'returncode': invalid.returncode,
                                                    'stdout': invalid.stdout, 'stderr': invalid.stderr})
            row['passed'] = True
        except (AssertionError, OSError, RuntimeError, ValueError) as error:
            row['error'] = f'{type(error).__name__}: {error}'
        report['results'].append(row)
        write_report(args.output, report)
    print(json.dumps({'passed': sum(row['passed'] for row in report['results']), 'engines': len(report['results'])}))
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
