#!/usr/bin/env python3
"""Inventory every original PHPT and support blob in one pinned upstream tree."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.build_upstream import COMMIT
from tools.capability_inventory import blobs

SUPPORT = ['tests/include', 'examples', 'docs/swoole-logo.svg', 'core-tests/fuzz/cases', 'README.md']


def inventory(source, group, module, requirements, *, pin=COMMIT):
    if not re.fullmatch(r'[A-Za-z0-9_-]+(?:/[A-Za-z0-9_-]+)*', group):
        raise ValueError('Require an upstream test group without traversal')
    command = ['git', '-C', str(source)]
    test_root = 'tests/' + group
    git = lambda *args: subprocess.check_output([*command, *args])
    if git('rev-parse', 'HEAD').decode().strip() != pin:
        raise ValueError('Upstream HEAD differs from the pinned source')
    if git('diff', '--name-only', pin, '--', test_root, *SUPPORT):
        raise ValueError('Pinned original test/support source has tracked changes')
    listing = git('ls-tree', '-r', '--name-only', pin, '--', test_root, *SUPPORT).decode().splitlines()
    tests, support = [], []
    for path, data in blobs(source, pin, listing).items():
        row = {'path': path, 'sha256': hashlib.sha256(data).hexdigest()}
        if path.startswith(test_root + '/') and path.endswith('.phpt'):
            tests.append({**row, 'module': module, 'requires': sorted(set(requirements))})
        else:
            support.append(row)
    if not tests:
        raise ValueError('Pinned test group is empty')
    return {'repository': 'https://github.com/swoole/swoole-src', 'commit': pin,
            'tests': tests, 'support_files': support,
            'inventory': {'cases': len(tests), 'support_blobs': len(support),
                          'group': group, 'tree': git('rev-parse', pin + ':' + test_root).decode().strip(),
                          'selection': 'Every original PHPT; no exclusions or assertion changes'}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--group', required=True)
    parser.add_argument('--module', required=True)
    parser.add_argument('--requires', action='append', default=['upstream_test_bootstrap'])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = inventory(args.source, args.group, args.module, args.requires)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['inventory']))


if __name__ == '__main__':
    main()
