#!/usr/bin/env python3
"""Shields.io endpoint JSON for the README's migration progress badges.

  badges.py --source PINNED_UPSTREAM --output DIR [--phpt PHPT_JSON --commit SHA]

replaced.json  upstream first-party lines (src/, ext-src/, include/; not the embedded
               PHP library) that the all-modules patch set deletes, over the pinned total:
               the minimal net diff after applying every patch, so glue a patch adds and a later
               one removes never counts.
graduated.json leaf modules that are always on (Phase 3).
phpt.json      pinned PHPTs passing on the hybrid in a merge-tier gate (--phpt: that
               run's gate output), with the commit it measured.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import GRADUATED, MODULES, apply_patch_verified, patches_for  # noqa: E402

LEAF = ('crc32', 'base64', 'error', 'framing', 'websocket', 'timer', 'protocols', 'buffer', 'http')
FIRST_PARTY = ('src/', 'ext-src/', 'include/')
NOT_COUNTED = {'ext-src/php_swoole_library.h'}  # generated embedded PHP library
SOURCE_SUFFIXES = ('.c', '.cc', '.cpp', '.h')


def first_party(path):
    return path.startswith(FIRST_PARTY) and path.endswith(SOURCE_SUFFIXES) and path not in NOT_COUNTED


def git(*args, cwd):
    return subprocess.run(['git', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def kept(source, patches):
    """{path: (kept, total)} first-party upstream lines after applying `patches`, in order, to `source`."""
    with tempfile.TemporaryDirectory(prefix='swr-badges-') as directory:
        tree = Path(directory)
        shutil.copytree(source, tree, dirs_exist_ok=True, ignore=shutil.ignore_patterns('.git'))
        git('init', '-q', cwd=tree)
        git('add', '-A', cwd=tree)
        git('-c', 'user.name=badges', '-c', 'user.email=badges@localhost', 'commit', '-qm', 'pin', cwd=tree)
        total = {path: len((tree / path).read_bytes().splitlines())
                 for path in git('ls-files', cwd=tree).splitlines() if first_party(path)}
        files = {path: (lines, lines) for path, lines in total.items()}
        for patch in patches:
            apply_patch_verified(tree, ROOT / patch['path'], patch['upstream_paths'], patch['sha256'])
        # Added glue must not earn credit by changing Git's greedy alignment.
        for line in git('diff', '--diff-algorithm=minimal', '--numstat', cwd=tree).splitlines():
            _, removed, path = line.split('\t')
            if path in files and removed != '-':
                files[path] = (total[path] - int(removed), total[path])
        return files


def replaced(files):
    """(deleted, total) first-party upstream lines over `kept`'s files."""
    return sum(total - lines for lines, total in files.values()), sum(total for _, total in files.values())


def all_module_patches():
    plan = json.loads((ROOT / 'migration/modules.json').read_text())
    selected = {'M00', *(module_id for module_id, _ in MODULES.values())}
    return patches_for([module for module in plan['modules'] if module['id'] in selected])


def phpt_passing(rows):
    """(hybrid passes, pinned PHPTs) from corpus/upstream/gate.py --output rows."""
    tests = {row['test'] for row in rows}
    passing = {row['test'] for row in rows if row['engine'] == 'hybrid' and row['status'] == 'passed'}
    return len(passing), len(tests)


def endpoint(label, message):
    return {'schemaVersion': 1, 'label': label, 'message': message, 'color': 'blue'}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--source', type=Path, required=True, help='Pinned upstream checkout')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--phpt', type=Path, help="A merge-tier run's phpt.json (gate rows)")
    parser.add_argument('--commit', help='The commit that run measured')
    parser.add_argument('--kept', action='store_true', help='Also list each file still built, with its kept upstream lines')
    args = parser.parse_args()
    if args.phpt and not args.commit:
        parser.error('--phpt needs --commit')
    args.output.mkdir(parents=True, exist_ok=True)
    files = kept(args.source, all_module_patches())
    for path, (lines, _) in sorted(files.items(), key=lambda item: -item[1][0]) if args.kept else ():
        if lines:
            print(f'{lines:6} {path}')
    deleted, total = replaced(files)
    badges = {
        'replaced': endpoint('upstream C++ replaced', f'{100 * deleted / total:.1f}% of {total / 1000:.0f}k lines'),
        'graduated': endpoint('graduated', f'{sum(m in GRADUATED for m in LEAF)} / {len(LEAF)} leaf modules'),
    }
    if args.phpt:
        passing, tests = phpt_passing(json.loads(args.phpt.read_text()))
        badges['phpt'] = endpoint('upstream PHPTs passing', f'{passing:,} / {tests:,} @ {args.commit[:7]}')
    for name, badge in badges.items():
        (args.output / (name + '.json')).write_text(json.dumps(badge) + '\n')
        print(f"{name}: {badge['message']}")


if __name__ == '__main__':
    main()
