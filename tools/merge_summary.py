#!/usr/bin/env python3
"""Markdown summary of a dev.py loop: phases per module and Cargo-engine cases per group.

Unsupported and skipped cases get their own columns; only `passed` counts as passed.
"""
import argparse
from collections import Counter, defaultdict
import json
import re
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.check_migration import MODULES
from tools.harness import ROOT

ALIASES = {'string': 'buffer'}
STATUSES = ['passed', 'failed', 'unsupported', 'skipped']


def module_of(phase):
    head = phase.split('-')[0]
    head = ALIASES.get(head, head)
    return head if head in MODULES else 'shared'


def summarize(loop):
    phases = defaultdict(Counter)
    for row in json.loads(loop.read_text())['phases']:
        phases[module_of(row['phase'])]['passed' if row.get('returncode') == 0 else 'failed'] += 1
    cases = defaultdict(Counter)
    for report in loop.parent.glob('*.json'):
        try:
            results = json.loads(report.read_text()).get('results')
        except (ValueError, AttributeError):
            continue
        for row in results if isinstance(results, list) else []:
            if not isinstance(row, dict) or 'status' not in row or row.get('engine') == 'upstream':
                continue
            module = row.get('module') or ('corpus' if 'project' in row else module_of(report.stem))
            cases[module][row['status'] if row['status'] in STATUSES else 'failed'] += 1
    # pytest JUnit reports: the engine is one of the test id's parameters.
    for report in loop.parent.glob('*.xml'):
        try:
            testcases = list(ET.parse(report).iter('testcase'))
        except ET.ParseError:
            continue
        for case in testcases:
            if 'upstream' in re.split(r'[][-]', case.get('name', '')):
                continue
            status = 'failed' if case.find('failure') is not None or case.find('error') is not None else \
                'skipped' if case.find('skipped') is not None else 'passed'
            cases[module_of(report.stem)][status] += 1
    return phases, cases


def markdown(phases, cases):
    lines = ['### Phases by module', '', '| Module | Passed | Failed |', '| --- | --- | --- |']
    lines += [f'| {module} | {phases[module]["passed"]} | {phases[module]["failed"]} |' for module in sorted(phases)]
    lines += ['', '### Cargo cases by group (original PHPT selection group, or corpus)', '',
              '| Group | ' + ' | '.join(s.capitalize() for s in STATUSES) + ' |', '| --- ' * (1 + len(STATUSES)) + '|']
    lines += [f'| {group} | ' + ' | '.join(str(cases[group][s]) for s in STATUSES) + ' |' for group in sorted(cases)]
    return '\n'.join(lines) + '\n\nUpstream is the control and is not counted.\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--loop', type=Path, help='loop.json; default the newest under target/reports/dev')
    args = parser.parse_args()
    loop = args.loop or max((ROOT / 'target/reports/dev').glob('*/loop.json'), key=lambda path: path.stat().st_mtime, default=None)
    if not loop:
        print('No dev.py loop report found.')
        return 1
    print(markdown(*summarize(loop)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
