#!/usr/bin/env python3
"""Edit a module's upstream patches as a Git branch: one commit per patch file.

  patches.py checkout MODULE   clone the pin to target/patches/MODULE, commit the patches the build
                               applies before MODULE's as one base commit, then commit each of MODULE's
  patches.py export MODULE     write each commit back to migration/patches/MODULE (new ones as <timestamp>-<subject>.patch)
  patches.py rehash OLD [NEW]  rewrite recorded hashes of upstream files changed from OLD to NEW (default: the pin)
  patches.py check             apply every module's patches, in build order, to the pin; fail on the first that doesn't
"""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import re
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_hybrid import module_patches, patches_for
from tools.build_upstream import COMMIT, VERSION
from tools.harness import ROOT


def git(work, *args):
    return subprocess.run(['git', '-C', str(work), '-c', 'user.name=patches', '-c', 'user.email=patches@localhost', *args],
                          check=True, capture_output=True, text=True).stdout


def clone_pin(work):
    reference = ROOT / 'target/upstream/src'
    source = [str(reference)] if (reference / '.git').exists() else ['--depth', '1', '--branch', VERSION,
                                                                     'https://github.com/swoole/swoole-src.git']
    subprocess.run(['git', '-c', 'advice.detachedHead=false', 'clone', '-q', *source, str(work)], check=True)


# The subject of the commit holding the patches applied before the module's own.
BASE = 'base: '


def base_patches(module):
    """The patches the build applies before `module`'s: those of every module ahead of it in
    build order. A module's patches are written against them."""
    plan = [m for m in json.loads((ROOT / 'migration/modules.json').read_text())['modules']
            if any(p.startswith('migration/patches/') for p in m['owned_paths'])]
    names = [next(p for p in m['owned_paths'] if p.startswith('migration/patches/')).rstrip('/').split('/')[-1] for m in plan]
    return patches_for(plan[:names.index(module)]) if module in names else []


def checkout(module, patches, work):
    if work.exists():
        raise SystemExit(f'{work} exists; export or delete it first')
    clone_pin(work)
    git(work, 'checkout', '-q', '-b', 'patches/' + module, COMMIT)
    base = base_patches(module)
    for patch in base:
        git(work, 'apply', '--index', str(ROOT / patch['path']))
    if base:
        git(work, 'commit', '-q', '-m', BASE + ' '.join(sorted({p['path'].split('/')[2] for p in base})))
    baseline = {}
    for patch in module_patches(patches):
        git(work, 'apply', '--index', str(patches / patch['path']))
        git(work, 'commit', '-q', '-m', patch['path'])
        baseline[patch['path']] = git(work, 'diff', '-U1', 'HEAD~', 'HEAD')
    (work / '.git/patches-baseline.json').write_text(json.dumps(baseline))
    print(work)


def export(patches, work):
    """Write one patch file per commit. A commit from checkout keeps its file name; a new commit
    gets `<author time, UTC>-<subject>.patch`, which sorts after every existing patch, so parallel
    branches adding patches to one module never collide."""
    # A commit whose diff is unchanged since checkout keeps its patch file byte for byte, so
    # frozen and generator-written patches survive a round trip.
    baseline = json.loads((work / '.git/patches-baseline.json').read_text())
    names = []
    for commit in git(work, 'rev-list', '--reverse', COMMIT + '..HEAD').split():
        subject, stamp = git(work, 'log', '-1', '--format=%s%n%at', commit).splitlines()
        if subject.startswith(BASE):
            continue
        name = subject
        if name not in baseline:
            moment = datetime.fromtimestamp(int(stamp), timezone.utc)
            slug = re.sub(r'[^a-z0-9]+', '-', subject.lower()).strip('-')[:48].rstrip('-') or 'patch'
            while True:
                name = f'{moment:%Y%m%dT%H%M%S}-{slug}.patch'
                if not names or name > names[-1]:
                    break
                moment += timedelta(seconds=1)
        # One context line, so patches from different modules to neighbouring lines still stack.
        body = git(work, 'diff', '-U1', commit + '~', commit)
        if baseline.get(name) != body:
            (patches / name).write_text(body)
        names.append(name)
    if names != sorted(names):
        raise SystemExit(f'Patch files apply in name order, but the commits are {names}: '
                         'add new commits after the existing ones, then export again')
    # Another PR may have added files since checkout; only prune our baseline.
    for stale in set(baseline) - set(names):
        (patches / stale).unlink(missing_ok=True)


def rehash(old, new):
    """Rewrite each recorded sha256 or git blob id of an upstream file from its OLD to its NEW value."""
    work = ROOT / 'target/patches/.rehash'
    shutil.rmtree(work, ignore_errors=True)
    subprocess.run(['git', 'init', '-q', str(work)], check=True)
    git(work, 'fetch', '-q', '--depth', '1', 'https://github.com/swoole/swoole-src.git', old, new)
    blob = lambda commit, path: hashlib.sha256(subprocess.run(['git', '-C', str(work), 'show', f'{commit}:{path}'],
                                                              check=True, capture_output=True).stdout).hexdigest()
    status = git(work, 'diff', '--name-status', '--no-renames', old, new).splitlines()
    mapping = {}
    for kind, path in (line.split('\t', 1) for line in status):
        if kind == 'M':
            mapping[blob(old, path)] = blob(new, path)
            mapping[git(work, 'rev-parse', f'{old}:{path}').strip()] = git(work, 'rev-parse', f'{new}:{path}').strip()
        elif kind == 'D':
            print(f'deleted upstream, review any reference: {path}')
    pattern = re.compile('|'.join(map(re.escape, mapping)))
    for name in subprocess.check_output(['git', 'ls-files', '-z'], cwd=ROOT, text=True).split('\0'):
        path = ROOT / name
        if not name or name.startswith('migration/patches/') or not path.is_file():
            continue
        try:
            text = path.read_text()
        except UnicodeDecodeError:
            continue
        updated = pattern.sub(lambda match: mapping[match.group()], text)
        if updated != text:
            path.write_text(updated)
            print(name)
    shutil.rmtree(work)


def check():
    """The all-modules patch set, in the order build_hybrid applies it, applies cleanly to the pin."""
    work = ROOT / 'target/patches/.check'
    shutil.rmtree(work, ignore_errors=True)
    clone_pin(work)
    git(work, 'checkout', '-q', COMMIT)
    plan = json.loads((ROOT / 'migration/modules.json').read_text())
    patches = patches_for([m for m in plan['modules'] if any(p.startswith('migration/patches/') for p in m['owned_paths'])])
    for patch in patches:
        result = subprocess.run(['git', '-C', str(work), 'apply', str(ROOT / patch['path'])], capture_output=True, text=True)
        if result.returncode:
            raise SystemExit(f"{patch['path']} does not apply over the patches before it:\n{result.stderr}")
    shutil.rmtree(work)
    print(f'{len(patches)} patches apply to {COMMIT[:12]}')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('command', choices=['checkout', 'export', 'rehash', 'check'])
    parser.add_argument('module', nargs='?', help='module, or OLD commit for rehash')
    parser.add_argument('new', nargs='?', default=COMMIT, help='rehash only: NEW commit')
    args = parser.parse_args()
    if args.command == 'check':
        return check()
    if not args.module:
        parser.error('module required')
    if args.command == 'rehash':
        return rehash(args.module, args.new)
    patches, work = ROOT / 'migration/patches' / args.module, ROOT / 'target/patches' / args.module
    if not patches.is_dir():
        raise SystemExit(f'No patch directory: {patches}')
    checkout(args.module, patches, work) if args.command == 'checkout' else export(patches, work)


if __name__ == '__main__':
    main()
