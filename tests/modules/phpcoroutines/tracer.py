#!/usr/bin/env python3
"""Compare the tracer's output (swoole_tracer.cc) across engines: profiling,
leak detection and blocking detection, each under its INI switch.

    python3 tests/modules/phpcoroutines/tracer.py --engine upstream=... --engine hybrid=...

Timings are normalized; leak reports are compared as a set of records, since
upstream reports sites in hash order.
"""
import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report  # noqa: E402

SCRIPT = Path(__file__).with_suffix('.php')
MODES = {
    'off': [],
    'profile': ['-d', 'swoole.profile=On'],
    'leak': ['-d', 'swoole.leak_detection=On'],
    'blocking': ['-d', 'swoole.blocking_detection=On', '-d', 'swoole.blocking_threshold=1000'],
}


def normalize(mode, output):
    output = re.sub(r'blocked for [\d,]+ us', 'blocked for N us', output)
    if mode == 'leak':
        head, *records = re.split(r'(?=\[Round#)', output)
        return head + ''.join(sorted(records))
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/phpcoroutines-tracer.json')
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    report = {'scope': 'swoole_tracer.cc: profiler, leak and blocking detection output', 'results': []}
    failed = 0
    for mode, ini in MODES.items():
        outputs = {}
        for engine in engines:
            command = engine.command(SCRIPT, mode)
            command[1:1] = ini
            result = run_isolated(command, timeout=60)
            outputs[engine.name] = (result.returncode, normalize(mode, result.stdout + result.stderr))
        values = list(outputs.values())
        same = all(value == values[0] for value in values)
        failed += not same
        report['results'].append({'mode': mode, 'match': same,
                                  'outputs': {name: {'exit': code, 'output': text} for name, (code, text) in outputs.items()}})
        print(('ok' if same else 'FAIL'), mode)
        if not same:
            for name, (code, text) in outputs.items():
                print(f'--- {name} (exit {code})\n{text}')
    report['passed'] = not failed
    write_report(args.output, report)
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
