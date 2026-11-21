#!/usr/bin/env python3
"""A whole-thread exit from a __destruct in a thread's request shutdown: the shutdown
finishes, freeing the objects still live (zend::Custom and hand-written free handlers)
and releasing the thread's arguments, and the thread exits with the exit's status.
Each mode must pass on the Cargo build wherever it passes on upstream."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import parse_engines, run_isolated, write_report

SCRIPT = Path(__file__).with_suffix('.php')
MODES = ['none', 'arguments', 'cookie', 'client', 'http_client', 'socket', 'scheduler']


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--php', required=True)
    p.add_argument('--engine', action='append', required=True, help='upstream=DSO and cargo=DSO')
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args()
    engines = {engine.name: engine for engine in parse_engines(a.engine, a.php)}
    report = dict(passed=False, results=[])
    for mode in MODES:
        expected = json.dumps([mode, 235], separators=(',', ':'))
        for name, engine in engines.items():
            command = engine.command(SCRIPT, mode)
            try:
                result = run_isolated(command, timeout=30)
                lines = result.stdout.splitlines()
                row = dict(exit=result.returncode, stdout=result.stdout, stderr=result.stderr,
                           passed=result.returncode == 0 and lines[-1:] == [expected])
            except subprocess.TimeoutExpired:
                row = dict(exit=None, timed_out=True, passed=False)
            report['results'].append(dict(engine=name, mode=mode, command=list(map(str, command)),
                                          expected=expected, **row))
    passed = {(row['engine'], row['mode']) for row in report['results'] if row['passed']}
    report['lost'] = [mode for mode in MODES if ('upstream', mode) in passed and ('cargo', mode) not in passed]
    report['passed'] = set(engines) == {'upstream', 'cargo'} and not report['lost']
    write_report(a.output, report)
    for row in report['results']:
        print(f"{row['engine']:9} {row['mode']:12} {'pass' if row['passed'] else 'FAIL'} exit={row['exit']}")
    print(json.dumps({key: report[key] for key in ['passed', 'lost']}))
    return int(not report['passed'])


if __name__ == '__main__':
    sys.exit(main())
