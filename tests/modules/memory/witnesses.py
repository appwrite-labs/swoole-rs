#!/usr/bin/env python3
"""Run the four PHP Table witnesses (not upstream PHPTs) on every engine and compare.

Each script prints JSON observations. Every engine must run each one cleanly and
print the same observations; upstream PHPTs for Table run in the merge tier's
PHPT gate.
"""
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import parse_engines, run_isolated

WITNESSES = [('table_state.php', {'table'}), ('table_fork.php', {'table', 'processes'}),
             ('table_storage.php', {'table', 'processes'}), ('table_clear_batches.php', {'table', 'processes'})]
MEMORY_BIT = 64


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--require-memory-bridge', action='append', default=[], metavar='ENGINE')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    report = {'engines': [], 'results': [], 'passed': False}
    for engine in engines:
        metadata = engine.metadata()
        report['engines'].append(metadata)
        if engine.name in args.require_memory_bridge and not metadata['rust_feature_bits'] & MEMORY_BIT:
            report['error'] = f'{engine.name} does not advertise memory feature bit {MEMORY_BIT}'
            continue
        for filename, required in WITNESSES:
            missing = required - set(metadata['capabilities'])
            row = {'engine': engine.name, 'name': filename}
            if missing:
                row.update(status='unsupported', missing=sorted(missing))
            else:
                run = run_isolated(engine.command(Path(__file__).with_name(filename)), timeout=30)
                row.update(status='passed' if run.returncode == 0 and not run.stderr else 'failed',
                           exit_code=run.returncode, stderr=run.stderr)
                if row['status'] == 'passed':
                    row['values'] = json.loads(run.stdout)
            report['results'].append(row)
    observed = {}
    for row in report['results']:
        observed.setdefault(row['name'], []).append(row.get('values'))
    report['observations_match'] = all(len(values) == len(engines) and all(v == values[0] for v in values)
                                       for values in observed.values())
    report['passed'] = ('error' not in report and report['observations_match']
                        and all(row['status'] == 'passed' for row in report['results']))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'observations_match': report['observations_match'],
                      'failed': [f"{r['engine']}:{r['name']}" for r in report['results'] if r['status'] != 'passed']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
