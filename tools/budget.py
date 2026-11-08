#!/usr/bin/env python3
"""Per-crate counts in non-test code that may only fall (tools/budget.json):

  unsafe   `unsafe` blocks, fns and impls
  maps     std hash and B-tree maps and sets, where a flat table belongs
  lists    LinkedList and VecDeque
  bools    arrays and Vecs of bool, where a bitset belongs
  caches   statics and types named for a cache: redesign the slow path instead

See docs/migration.md, Data layout.

  budget.py           fail if any crate is above its baseline
  budget.py --update  lower the baseline to the current counts (never raise it)

Test code is not counted: a `#[cfg(test)]` inline module, a file a `#[cfg(test)] mod`
loads, and anything under a tests/ or benches/ directory.
"""
import argparse
import json
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / 'tools/budget.json'
KINDS = {
    'unsafe': re.compile(r'\bunsafe\s*(?:\{|(?:extern\s+"[^"]*"\s+)?fn\b|impl\b)'),
    'maps': re.compile(r'\b(?:HashMap|HashSet|BTreeMap|BTreeSet)\b'),
    'lists': re.compile(r'\b(?:LinkedList|VecDeque)\b'),
    'bools': re.compile(r'Vec<bool>|\[bool;'),
    'caches': re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:static|struct|enum|type)\s+\w*(?i:cache)'),
}
TEST_MOD = re.compile(r'^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])')
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')


def count_file(path, test_files):
    """{kind: matches} in `path` outside #[cfg(test)] modules; adds files those modules load to `test_files`."""
    count = dict.fromkeys(KINDS, 0)
    skip_until, pending, pending_path = None, False, None
    for line in path.read_text(errors='replace').splitlines():
        if skip_until is not None:
            if line.rstrip() == skip_until:
                skip_until = None
            continue
        if pending:
            if match := PATH_ATTR.search(line):
                pending_path = match.group(1)
            if match := TEST_MOD.match(line):
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
            if line.strip().startswith('#['):
                continue
            pending = False  # #[cfg(test)] on an item that is not a module: counted
        if line.strip() == '#[cfg(test)]':
            pending, pending_path = True, None
            continue
        code = line.split('//', 1)[0]
        for kind, pattern in KINDS.items():
            count[kind] += len(pattern.findall(code))
    return count


def counts(root=ROOT):
    """{kind: {crate: matches}} over crates/*/ non-test Rust sources."""
    result = {kind: {} for kind in KINDS}
    for crate in sorted(p for p in (root / 'crates').iterdir() if (p / 'Cargo.toml').exists()):
        files = sorted(p for p in crate.rglob('*.rs')
                       if not {'tests', 'benches', 'target'} & set(p.relative_to(crate).parts[:-1]))
        test_files, per_file = set(), {}
        for path in files:
            per_file[path.resolve()] = count_file(path, test_files)
        for kind in KINDS:
            result[kind][crate.name] = sum(n[kind] for path, n in per_file.items() if path not in test_files)
    return result


def check(current, baseline):
    """(crates above their baseline, crates below it) as {crate: (current, baseline)}."""
    above = {c: (n, baseline.get(c, 0)) for c, n in current.items() if n > baseline.get(c, 0)}
    below = {c: (n, baseline[c]) for c, n in current.items() if c in baseline and n < baseline[c]}
    return above, below


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--update', action='store_true', help='Lower the baseline to the current counts')
    args = parser.parse_args()
    current = counts()
    baseline = json.loads(BASELINE.read_text()) if BASELINE.exists() else {}
    if args.update:
        baseline = {kind: {c: min(n, baseline.get(kind, {}).get(c, n)) for c, n in crates.items()}
                    for kind, crates in current.items()}
        BASELINE.write_text(json.dumps(baseline, indent=1, sort_keys=True) + '\n')
    failed = False
    for kind, crates in current.items():
        limits = baseline.get(kind, {})
        above, below = check(crates, limits)
        failed = failed or bool(above)
        for crate, (n, limit) in sorted(above.items()):
            print(f'{crate}: {n} {kind}, above the baseline of {limit}', file=sys.stderr)
        for crate, (n, limit) in sorted(below.items()):
            print(f'{crate}: {n} {kind}, below the baseline of {limit}: '
                  'run tools/budget.py --update and commit it')
        print(f'{kind}: {sum(crates.values())} (baseline {sum(limits.values())})')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
