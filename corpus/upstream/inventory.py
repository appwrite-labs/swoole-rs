#!/usr/bin/env python3
"""Write a run.py manifest for every pinned upstream PHPT, or one shard of them, of every
family or the --module ones.

Every other file under tests/ (fixtures, helpers) becomes a support file, as do
the fixtures tests reach outside tests/ (examples/, package.xml, the HTTP fuzz
cases), so tests run with their original surroundings.
Tests and expectations are unchanged.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.build_upstream import COMMIT
from tools.harness import ROOT


def shard(paths, index, count):
    """Every count-th path from index (1-based), so slow families spread across shards."""
    return [path for position, path in enumerate(sorted(paths)) if position % count == index - 1]


# Outside tests/, but read by tests: config.php's TEST_IMAGE*, DOCUMENT_ROOT and
# SOURCE_ROOT_PATH fixtures, and the static handlers' document_root (examples/).
FIXTURES = ['README.md', 'docs/swoole-logo.svg', 'examples', 'package.xml', 'core-tests/fuzz/cases']


def blobs(source):
    listing = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', COMMIT, 'tests', *FIXTURES],
                                      text=True)
    for line in listing.splitlines():
        meta, path = line.split('\t', 1)
        if meta.split()[1] == 'blob':
            yield path


def digest(source, path):
    return hashlib.sha256(subprocess.check_output(['git', '-C', str(source), 'cat-file', 'blob', f'{COMMIT}:{path}'])).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src')
    parser.add_argument('--shard', default='1/1', help='INDEX/COUNT, 1-based')
    parser.add_argument('--module', action='append', help='Only this PHPT family (tests/<module>/), before sharding; repeatable')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    index, count = map(int, args.shard.split('/'))
    if not 1 <= index <= count:
        parser.error('--shard must be INDEX/COUNT with 1 <= INDEX <= COUNT')
    paths = list(blobs(args.source))
    tests = [path for path in paths if path.endswith('.phpt')]
    unknown = set(args.module or []) - {path.split('/')[1] for path in tests}
    if unknown:
        parser.error(f'Unknown modules: {", ".join(sorted(unknown))}')
    tests = shard([path for path in tests if not args.module or path.split('/')[1] in args.module], index, count)
    support = [path for path in paths if not path.endswith('.phpt') and not path.startswith('tests/include/')]
    manifest = {'repository': 'https://github.com/swoole/swoole-src', 'commit': COMMIT,
                'tests': [{'path': path, 'sha256': digest(args.source, path), 'module': path.split('/')[1],
                           'requires': []} for path in tests],
                'support_files': [{'path': path, 'sha256': digest(args.source, path)} for path in support]}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(manifest, indent=1) + '\n')
    print(f'{len(tests)} tests, {len(support)} support files -> {args.output}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
