"""Compare error_handler_bailout.php's output and exit code on each engine with upstream's.

    python3 tests/modules/zend/run.py --engine upstream=<swoole.so> --engine cargo=<swoole.so> --output <report.json>
"""
import argparse
import json
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated

CASES = ['atomic_coercion', 'process_signal', 'table_column', 'table_conversion', 'timer_after',
         'event_del', 'cookie_name', 'http_client_file', 'server_coro_accept']
SCRIPT = Path(__file__).with_name('error_handler_bailout.php')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    oracle = next(engine for engine in engines if engine.name == 'upstream')
    report = dict(engines=[engine.metadata() for engine in engines], cases=[])
    for case in CASES:
        for mode in ['exit', 'fatal']:
            row = dict(case=case, mode=mode)
            for engine in engines:
                result = run_isolated(engine.command(SCRIPT, case, mode), timeout=30)
                row[engine.name] = dict(exit=result.returncode, output=result.stdout + result.stderr)
            row['passed'] = all(row[engine.name] == row[oracle.name] for engine in engines)
            report['cases'].append(row)
            print(('ok' if row['passed'] else 'FAIL') + f' {case} {mode}')
            if not row['passed']:
                print(json.dumps(row, indent=2))
    report['passed'] = all(row['passed'] for row in report['cases'])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
