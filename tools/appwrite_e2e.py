#!/usr/bin/env python3
"""Compare Appwrite e2e JUnit reports: the Cargo build against stock (upstream) Swoole.

Reads DIR/junit-<variant>-<service>/junit.xml as appwrite-e2e.yml uploads them.
Paratest's merged report keeps complete top-level totals but drops some per-worker
testcases, so services are gated on totals: a service regresses when its cargo
report is missing, ran a different number of tests, or has more failures and
errors than upstream's. Failing testcase names that the report kept are listed.
Prints a Markdown summary.
"""
import argparse
from pathlib import Path
import sys
import xml.etree.ElementTree as ET


def report(path):
    """(tests, failed, skipped, failing testcase names) for one JUnit file."""
    root = ET.parse(path).getroot()
    top = root.find('testsuite') if root.tag == 'testsuites' else root
    failing = sorted(f"{case.get('class') or case.get('classname')}::{case.get('name')}"
                     for case in root.iter('testcase')
                     if case.find('failure') is not None or case.find('error') is not None)
    total = {key: int(top.get(key, 0)) for key in ('tests', 'failures', 'errors', 'skipped')}
    return total['tests'], total['failures'] + total['errors'], total['skipped'], failing


def load(directory):
    """{variant: {service: report}}."""
    reports = {}
    for path in sorted(Path(directory).glob('junit-*/junit.xml')):
        _, variant, service = path.parent.name.split('-', 2)
        reports.setdefault(variant, {})[service] = report(path)
    return reports


def compare(reports):
    """(rows, regressions, upstream failures): rows are (service, upstream, cargo)."""
    rows, regressions, flaky = [], [], []
    cargo = reports.get('cargo', {})
    for service, stock in sorted(reports.get('upstream', {}).items()):
        ours = cargo.get(service)
        rows.append((service, stock, ours))
        if stock[1]:
            flaky.append(f'{service}: {stock[1]} failing on upstream' + ''.join(f'\n  - {t}' for t in stock[3]))
        if ours is None:
            regressions.append(f'{service}: no cargo report')
        elif ours[0] != stock[0] or ours[1] > stock[1]:
            new = [t for t in ours[3] if t not in stock[3]]
            regressions.append(f'{service}: {ours[0]} tests, {ours[1]} failing (upstream {stock[0]}, {stock[1]})'
                               + ''.join(f'\n  - {t}' for t in new))
    return rows, regressions, flaky


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    rows, regressions, flaky = compare(load(args.directory))
    print('| service | upstream tests / failing / skipped | cargo tests / failing / skipped |\n| --- | --- | --- |')
    for service, stock, ours in rows:
        print(f"| {service} | {' / '.join(map(str, stock[:3]))} | {' / '.join(map(str, ours[:3])) if ours else 'missing'} |")
    for title, items in [('Regressions', regressions), ('Failing on upstream (not gated)', flaky)]:
        print(f'\n### {title}: {len(items)}')
        print(''.join(f'- {item}\n' for item in items), end='')
    return 1 if regressions or not rows else 0


if __name__ == '__main__':
    sys.exit(main())
