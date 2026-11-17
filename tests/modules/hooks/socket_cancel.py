#!/usr/bin/env python3
"""Cancel and close socket waits through their parks, from coroutines and reactor callbacks."""
import argparse
import json
from pathlib import Path
import sys
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report  # noqa: E402

EXPECTED = 'socket cancellation passed\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = {'passed': False, 'scope': __doc__, 'results': []}
    try:
        for engine in parse_engines(args.engine, args.php, args.php_extension):
            for mode in ['coroutine', 'reactor']:
                result = run_isolated([*engine.command(Path(__file__).with_suffix('.php')), mode], timeout=15)
                report['results'].append({'engine': engine.name, 'mode': mode, 'exit': result.returncode,
                                          'stdout': result.stdout, 'stderr': result.stderr})
                if result.returncode or result.stderr or result.stdout != EXPECTED:
                    raise ValueError(f'{engine.name}/{mode}: exit {result.returncode}; outputs in the report')
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_report(args.output, report)
    print(json.dumps({key: report.get(key) for key in ['passed', 'error']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
