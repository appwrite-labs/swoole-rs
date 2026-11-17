#!/usr/bin/env python3
"""Counts in non-test code whose workspace totals may not rise above the base commit's
(#4733): an item that moves between crates isn't an item that was added, so a crate may
rise by what others fall, and a fold or a new crate moves what it holds.

  unsafe   `unsafe` blocks, fns and impls
  maps     std hash and B-tree maps and sets, where a flat table belongs
  lists    LinkedList and VecDeque
  bools    arrays and Vecs of bool, where a bitset belongs
  caches   statics and types named for a cache: redesign the slow path instead
  mints    safe fns that hand out `&mut` from `&self` or a `Copy` handle (#3554): two calls
           give two live `&mut` to one place in safe code. Return a raw pointer or
           per-field access, or make the fn `unsafe` with the aliasing rule in `# Safety`
  fakes    `cfg(...not(test)...)`: code tests and Miri don't run, usually because a
           test build swaps in a stand-in. Pass the dependency in (an ops table, a
           closure, a trait) so tests run the shipped body (#3556)
  casts    cast lints a `#[expect]` names: convert with `c_cast::<T>()` (corebase) where
           upstream's C converts, or keep the value in the type it is used as (#3564)
  cells    `RefCell` types (fields, statics, thread-locals): a borrow held across a yield
           or a PHP callback is re-entered, and the process aborts. Make the state a
           plain field of the handle that owns it, borrowed `&mut`, or a `Cell` taken and
           put back (#3565). A `RefCell::new` whose type is inferred isn't counted
  statuses integer-returning bodies (`i32`, `c_int`, `i64`, `isize`), including extern
           wrappers; foreign declarations and callback types have no body to migrate
  reprs    `#[repr(C)]` layouts, including C combined with other representation hints
  derefs   safe fns, safe `extern "C"` callbacks included, with a raw-pointer parameter an
           `unsafe` block in the body names: safe code can pass any pointer. Clippy's
           `not_unsafe_ptr_arg_deref` checks only exported fns (#4275). Make the fn
           `unsafe` with the pointer's contract in `# Safety`, or take a reference
  externs  foreign fns declared `safe fn` in an `unsafe extern` block with a raw-pointer
           parameter: safe code can call them with any pointer, so declaring a callee
           safe can't trade `unsafe` away (#4761). Take a reference or a typed handle, or
           leave the declaration unsafe
  clines   lines of C (`.c`, `.h`) under the crate's src/, which crates/oracle/upstream
           isn't in. A shim that fits no reason in docs/zend.md's "What stays C" is a port
           waiting to happen, so a rise passes only when the PR body has a line
           `Stays C: <reason>` naming those reasons (the PR tier reads the body it had when
           it ran: push again after editing; locally, set PR_BODY)
  unforbidden  1 unless the crate's lib.rs has `#![forbid(unsafe_code)]`, so a crate
               the compiler verifies safe stays that way. Checked per crate, not in
               total: a crate new since REF with no unsafe, counted or in an extern
               block or attribute, must forbid `unsafe_code`

See docs/migration.md, Data layout.

  budget.py [--base REF]   fail if any workspace total counts more than at REF (default:
                           the merge base with origin/main; the PR tier passes HEAD^1)

There is no baseline file: every PR that removes an item would edit it, so
parallel PRs conflicted on it. A failing total lists the crates that rose, to show
where the items went.

Test code is not counted: a `#[cfg(test)]` inline module, a file a `#[cfg(test)] mod`
loads, and anything under a tests/ or benches/ directory.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SAFE_FN = re.compile(r'(\bunsafe\s+)?(?:extern\s+"[^"]*"\s+)?\bfn\s+\w+\s*(?:<[^{;()]*?>)?\s*\(')
RAW_PARAM = re.compile(r'(?:mut\s+)?(\w+)\s*:\s*(?:Option<\s*)?\*\s*(?:const|mut)\b')
UNSAFE_BLOCK = re.compile(r'\bunsafe\s*\{')
UNSAFE_EXTERN = re.compile(r'\bunsafe\s+extern\s*(?:"[^"]*"\s*)?\{')
SAFE_EXTERN_FN = re.compile(r'\bsafe\s+fn\s+\w+\s*(?:<[^{;()]*?>)?\s*\(')


def closing(code, i, open_, close):
    """The index after the `close` that balances an `open_` just before `i`."""
    depth = 1
    while i < len(code) and depth:
        depth += (code[i] == open_) - (code[i] == close)
        i += 1
    return i


def params(code, i):
    """The parameters of the parenthesis that opens just before `i`, split at top-level
    commas (a callback type's `*mut` is not a pointer the fn takes), and the index past
    its close."""
    end = closing(code, i, '(', ')')
    split, depth = [''], 0
    for ch in code[i:end - 1].replace('->', '-'):
        depth += (ch in '(<[') - (ch in ')>]')
        if ch == ',' and depth == 0:
            split.append('')
        else:
            split[-1] += ch
    return [p.strip() for p in split], end


def derefs(code):
    """The safe fns in `code` whose `unsafe` blocks name one of their raw-pointer parameters."""
    found = []
    for fn in SAFE_FN.finditer(code):
        split, end = params(code, fn.end())
        names = [m.group(1) for p in split if (m := RAW_PARAM.match(p))]
        start = re.compile(r'[{;]').search(code, end)
        if fn.group(1) or not names or not start or start.group() == ';':
            continue
        body = code[start.end():closing(code, start.end(), '{', '}')]
        blocks = ''.join(body[b.end():closing(body, b.end(), '{', '}')] for b in UNSAFE_BLOCK.finditer(body))
        if any(re.search(rf'\b{name}\b', blocks) for name in names):
            found.append(fn.group())
    return found


def externs(code):
    """The foreign fns `code` declares `safe` in an `unsafe extern` block with a raw-pointer
    parameter: safe code can call them with any pointer (#4761)."""
    found = []
    for block in UNSAFE_EXTERN.finditer(code):
        body = code[block.end():closing(code, block.end(), '{', '}')]
        for fn in SAFE_EXTERN_FN.finditer(body):
            if any(RAW_PARAM.match(p) for p in params(body, fn.end())[0]):
                found.append(fn.group())
    return found


KINDS = {
    'unsafe': re.compile(r'\bunsafe\s*(?:\{|(?:extern\s+"[^"]*"\s+)?fn\b|impl\b)'),
    'maps': re.compile(r'\b(?:HashMap|HashSet|BTreeMap|BTreeSet)\b'),
    'lists': re.compile(r'\b(?:LinkedList|VecDeque)\b'),
    'bools': re.compile(r'Vec<bool>|\[bool;'),
    'caches': re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:static|struct|enum|type)\s+\w*(?i:cache)'),
    # A one-line safe signature returning `&mut` or `Option<&mut>` with no `&mut` anywhere in its
    # parameters, which may nest one level of parentheses (`()`, a tuple, a fn type). A `&mut`
    # there, even in a tuple or a callback's type, could be what it lends, so it isn't counted.
    'mints': re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?fn\s+\w+(?:<[^>]*>)?"
                        r"\((?:(?!&\s*(?:'\w+\s+)?mut\b)(?:[^()]|\((?:(?!&\s*(?:'\w+\s+)?mut\b)[^()])*\)))*\)"
                        r"\s*->\s*(?:Option<)?&(?:'\w+\s+)?mut\b"),
    'fakes': re.compile(r'\bcfg!?\([^\n]*\bnot\(\s*test\s*\)'),
    'casts': re.compile(r'\bclippy::cast_(?:possible_truncation|sign_loss|possible_wrap|precision_loss)\b'),
    'cells': re.compile(r'\bRefCell\s*<'),
    'statuses': re.compile(r'\)\s*->\s*(?:i32|c_int|i64|isize)\s*(?:where\b[^;{]*)?\{'),
    'reprs': re.compile(r'#\s*\[\s*repr\s*\([^\]]*\bC\b[^\]]*\)\s*\]'),
    'derefs': derefs,
    'externs': externs,
}
MULTILINE = ('statuses', 'reprs', 'derefs', 'externs')
# What `forbid(unsafe_code)` rejects and KINDS['unsafe'] doesn't count.
UNCOUNTED = re.compile(r'\bunsafe\s+extern\b|#\[unsafe\(')
FORBID = re.compile(r'^#!\[forbid\([^)]*\bunsafe_code\b', re.M)
TEST_MOD = re.compile(r'^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])')
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
C_SUFFIXES = ('.c', '.h')


def count_file(path, test_files):
    """{kind: matches} in `path` outside #[cfg(test)] modules; adds files those modules load to `test_files`."""
    count = dict.fromkeys(KINDS, 0)
    source = []
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
        source.append(code)
        for kind, pattern in KINDS.items():
            if kind not in MULTILINE:
                count[kind] += len(pattern.findall(code))
    code = '\n'.join(source)
    for kind in MULTILINE:
        find = KINDS[kind]
        count[kind] = len(find(code) if callable(find) else find.findall(code))
    return count


def counts(root=ROOT):
    """{kind: {crate: matches}} over crates/*/ non-test Rust sources."""
    result = {kind: {} for kind in [*KINDS, 'clines', 'unforbidden']}
    for crate in sorted(p for p in (root / 'crates').iterdir() if (p / 'Cargo.toml').exists()):
        files = sorted(p for p in crate.rglob('*.rs')
                       if not {'tests', 'benches', 'target'} & set(p.relative_to(crate).parts[:-1]))
        test_files, per_file = set(), {}
        for path in files:
            per_file[path.resolve()] = count_file(path, test_files)
        for kind in KINDS:
            result[kind][crate.name] = sum(n[kind] for path, n in per_file.items() if path not in test_files)
        result['clines'][crate.name] = sum(len(p.read_text(errors='replace').splitlines())
                                           for p in (crate / 'src').rglob('*') if p.suffix in C_SUFFIXES)
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


def rises_by_crate(current, base):
    """{crate: (current, base)} for crates that count more than at the base, a crate new
    since the base counting from zero."""
    return {c: (n, base.get(c, 0)) for c, n in current.items() if n > base.get(c, 0)}


def must_forbid(current, base, root=ROOT):
    """{crate: (1, 0)} for crates new since the base that hold no unsafe, counted or
    UNCOUNTED, and don't forbid it."""
    return {c: (1, 0) for c, n in current['unforbidden'].items()
            if n and c not in base and not current['unsafe'][c]
            and not any(UNCOUNTED.search(line.split('//', 1)[0])
                        for p in (root / 'crates' / c).rglob('*.rs')
                        for line in p.read_text(errors='replace').splitlines())}


def stays_c(body, zend=ROOT / 'docs/zend.md'):
    """The reasons `body`'s `Stays C:` lines name, when each is one docs/zend.md's "What
    stays C" lists; else none."""
    section = zend.read_text().split('## What stays C', 1)[1].split('\n## ', 1)[0]
    known = set(re.findall(r'^- \*\*(\w+)\*\*:', section, re.M))
    named = [word for line in re.findall(r'^Stays C:(.*)$', body, re.M) for word in re.findall(r'\w+', line)]
    return named if named and set(named) <= known else []


def failures(current, base, body='', root=ROOT):
    """{kind: [line]} for each count that rose: its workspace total and the crates that
    rose, or for `unforbidden` the crates. A `clines` rise passes when `body` says why
    it stays C."""
    failed = {}
    for kind, crates in current.items():
        if kind == 'unforbidden':
            rises = above(crates, base[kind]) | must_forbid(current, base[kind], root)
            if rises:
                failed[kind] = [f'{c}: {n} {kind}, up from {was}' for c, (n, was) in sorted(rises.items())]
            continue
        total, was = sum(crates.values()), sum(base[kind].values())
        if total <= was or (kind == 'clines' and stays_c(body)):
            continue
        failed[kind] = [f'{kind}: {total} in the workspace, up from {was}',
                        *(f'  {c}: {n}, up from {before}'
                          for c, (n, before) in sorted(rises_by_crate(crates, base[kind]).items()))]
    return failed


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--base', help='Commit to compare against (default: merge base with origin/main)')
    args = parser.parse_args()
    base_ref = args.base or subprocess.run(['git', '-C', str(ROOT), 'merge-base', 'HEAD', 'origin/main'],
                                           check=True, capture_output=True, text=True).stdout.strip()
    current, base = counts(), counts_at(base_ref)
    body = os.environ.get('PR_BODY', '')
    if (reasons := stays_c(body)) and sum(current['clines'].values()) > sum(base['clines'].values()):
        print(f'clines may rise: the PR says it stays C ({", ".join(reasons)})', file=sys.stderr)
    failed = failures(current, base, body)
    for kind, crates in current.items():
        print(f'{kind}: {sum(crates.values())} (base {sum(base[kind].values())})')
        for line in failed.get(kind, []):
            print(line, file=sys.stderr)
    if failed:
        print(f'budget: counts rose over {base_ref[:12]}', file=sys.stderr)
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
