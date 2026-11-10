#!/usr/bin/env python3
"""Per-crate counts in non-test code that may not rise above the base commit's:

  unsafe   `unsafe` blocks, fns and impls (crates/zend, the safe layer callers move
           onto, may rise while the workspace total falls, and an `X-ffi` crate, the C ABI
           over crate X, by as much as X falls while the total doesn't rise)
  maps     std hash and B-tree maps and sets, where a flat table belongs
  lists    LinkedList and VecDeque
  bools    arrays and Vecs of bool, where a bitset belongs
  caches   statics and types named for a cache: redesign the slow path instead
  unforbidden  1 unless the crate's lib.rs has `#![forbid(unsafe_code)]`, so a crate
               the compiler verifies safe stays that way

See docs/migration.md, Data layout.

  budget.py [--base REF]   fail if any crate counts more than at REF (default: the
                           merge base with origin/main; the PR tier passes HEAD^1)

There is no baseline file: every PR that removes an item would edit it, so
parallel PRs conflicted on it. A crate new since REF counts from zero once the
workspace total rises, so code may move into a new crate but not arrive with one; a
new crate with no unsafe, counted or in an extern block or attribute, must forbid
`unsafe_code`.

Test code is not counted: a `#[cfg(test)]` inline module, a file a `#[cfg(test)] mod`
loads, and anything under a tests/ or benches/ directory.
"""
import argparse
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
KINDS = {
    'unsafe': re.compile(r'\bunsafe\s*(?:\{|(?:extern\s+"[^"]*"\s+)?fn\b|impl\b)'),
    'maps': re.compile(r'\b(?:HashMap|HashSet|BTreeMap|BTreeSet)\b'),
    'lists': re.compile(r'\b(?:LinkedList|VecDeque)\b'),
    'bools': re.compile(r'Vec<bool>|\[bool;'),
    'caches': re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:static|struct|enum|type)\s+\w*(?i:cache)'),
}
# What `forbid(unsafe_code)` rejects and KINDS['unsafe'] doesn't count.
UNCOUNTED = re.compile(r'\bunsafe\s+extern\b|#\[unsafe\(')
FORBID = re.compile(r'^#!\[forbid\([^)]*\bunsafe_code\b', re.M)
TEST_MOD = re.compile(r'^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])')
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def count_file(path, test_files):
    """{kind: matches} in `path` outside #[cfg(test)] modules; adds files those modules load to `test_files`."""
    count = dict.fromkeys(KINDS, 0)
    skip_until, pending, pending_path, depth = None, False, None, 0
    for line in path.read_text(errors='replace').splitlines():
        if skip_until is not None:
            if line.rstrip() == skip_until:
                skip_until = None
            continue
        if pending:
            if match := PATH_ATTR.search(line):
                pending_path = match.group(1)
            if depth == 0 and (match := TEST_MOD.match(line)):
                pending = False
                if match.group(3) == '{':
                    skip_until = match.group(1) + '}'
                else:
                    name = pending_path or match.group(2) + '.rs'
                    base = path.parent if pending_path or path.name in ('lib.rs', 'main.rs', 'mod.rs') \
                        else path.parent / path.stem
                    test_files.add((base / name).resolve())
                    test_files.add((base / match.group(2) / 'mod.rs').resolve())
                continue
            # Another attribute, which may span lines, before the module.
            if depth or line.strip().startswith('#['):
                depth += line.count('[') - line.count(']')
                continue
            pending = False  # #[cfg(test)] on an item that is not a module: counted
        if line.strip() == '#[cfg(test)]':
            pending, pending_path, depth = True, None, 0
            continue
        code = line.split('//', 1)[0]
        for kind, pattern in KINDS.items():
            count[kind] += len(pattern.findall(code))
    return count


def counts(root=ROOT):
    """{kind: {crate: matches}} over crates/*/ non-test Rust sources."""
    result = {kind: {} for kind in [*KINDS, 'unforbidden']}
    for crate in sorted(p for p in (root / 'crates').iterdir() if (p / 'Cargo.toml').exists()):
        files = sorted(p for p in crate.rglob('*.rs')
                       if not {'tests', 'benches', 'target'} & set(p.relative_to(crate).parts[:-1]))
        test_files, per_file = set(), {}
        for path in files:
            per_file[path.resolve()] = count_file(path, test_files)
        for kind in KINDS:
            result[kind][crate.name] = sum(n[kind] for path, n in per_file.items() if path not in test_files)
        result['unforbidden'][crate.name] = 0 if FORBID.search((crate / 'src/lib.rs').read_text()) else 1
    return result


def counts_at(ref, root=ROOT):
    """counts() of `ref`'s crates/ tree."""
    archive = subprocess.run(['git', '-C', str(root), 'archive', ref, 'crates'],
                             check=True, capture_output=True).stdout
    with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryFile() as tar:
        tar.write(archive)
        tar.seek(0)
        with tarfile.open(fileobj=tar) as tree:
            tree.extractall(directory, filter='data')
        return counts(Path(directory))


def above(current, base):
    """{crate: (current, base)} for crates that existed at the base and count more now."""
    return {c: (n, base[c]) for c, n in current.items() if c in base and n > base[c]}


def boundary_moves(rises, current, base):
    """`rises` without each `X-ffi` crate whose rise crate X's fall covers: unsafe that
    moved from X to the C ABI over it. Only while the workspace total doesn't rise, so a
    fall into a new crate can't cover the rise as well."""
    def covered(crate, n, was):
        safe = crate.removesuffix('-ffi')
        return safe != crate and n - was <= base.get(safe, 0) - current.get(safe, 0)
    if sum(current.values()) > sum(base.values()):
        return rises
    return {c: (n, was) for c, (n, was) in rises.items() if not covered(c, n, was)}


def arrivals(current, base):
    """{crate: (current, 0)} for crates new since the base, when the workspace total
    rose: what a new crate holds must have left the crates that were there."""
    if sum(current.values()) <= sum(base.values()):
        return {}
    return {c: (n, 0) for c, n in current.items() if c not in base and n}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--base', help='Commit to compare against (default: merge base with origin/main)')
    args = parser.parse_args()
    base_ref = args.base or subprocess.run(['git', '-C', str(ROOT), 'merge-base', 'HEAD', 'origin/main'],
                                           check=True, capture_output=True, text=True).stdout.strip()
    current, base = counts(), counts_at(base_ref)
    failed = False
    for kind, crates in current.items():
        rises = above(crates, base[kind])
        if kind == 'unsafe':
            rises = boundary_moves(rises, crates, base[kind])
            if sum(crates.values()) < sum(base[kind].values()):
                rises.pop('zend', None)
        if kind == 'unforbidden':
            rises |= {c: (1, 0) for c, n in crates.items()
                      if n and c not in base[kind] and not current['unsafe'][c]
                      and not any(UNCOUNTED.search(line.split('//', 1)[0])
                                  for p in (ROOT / 'crates' / c).rglob('*.rs')
                                  for line in p.read_text(errors='replace').splitlines())}
        else:
            rises |= arrivals(crates, base[kind])
        failed = failed or bool(rises)
        for crate, (n, was) in sorted(rises.items()):
            print(f'{crate}: {n} {kind}, up from {was} at {base_ref[:12]}', file=sys.stderr)
        print(f'{kind}: {sum(crates.values())} (base {sum(base[kind].values())})')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
