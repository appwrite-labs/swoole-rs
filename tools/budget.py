#!/usr/bin/env python3
"""Counts in non-test code whose workspace totals may not rise above the base commit's
(#4733): an item that moves between crates isn't an item that was added, so a crate may
rise by what others fall, and a fold or a new crate moves what it holds.

  unsafe   `unsafe` blocks, fns and impls; a fn pointer's type is not a fn
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

The Rust counts are ast-grep rules over tree-sitter's syntax tree (the `ast-grep-py`
package), so a comment or a string never counts. The tree has only tokens inside a
macro's body: there `unsafe`, `bools`, `caches`, `cells`, `statuses` and `reprs` match
runs of tokens, `maps`, `lists` and `casts` match a name wherever it is, and `mints`,
`fakes` and `derefs` don't see in.
"""
import argparse
from concurrent.futures import ProcessPoolExecutor
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile

try:
    from ast_grep_py import SgRoot
except ImportError:
    sys.exit('tools/budget.py needs the ast-grep-py package: see docs/development.md, Linux environment')

ROOT = Path(__file__).resolve().parents[1]


def named(*names):
    """A path or type segment that is one of `names`."""
    return {'any': [{'kind': 'identifier'}, {'kind': 'type_identifier'}], 'regex': f'^(?:{"|".join(names)})$'}


def option(inner):
    """`inner`, or an `Option` of it."""
    return {'any': [inner, {'kind': 'generic_type', 'regex': r'^Option\b',
                            'has': {'kind': 'type_arguments', 'has': inner}}]}


def attribute(name, **rule):
    """The attribute `name`, as in `#[name(...)]`."""
    return {'kind': 'attribute', 'all': [{'has': {'kind': 'identifier', 'regex': f'^{name}$'}}, *([rule] if rule else [])]}


def tokens(*run):
    """The first of a run of tokens in a macro's body, which the grammar leaves as a tree
    of tokens. Each is a token's text or a rule."""
    rule = {}
    for item in reversed(run):
        rule = {**({'regex': f'^(?:{item})$'} if isinstance(item, str) else item), **({'precedes': rule} if rule else {})}
    return {**rule, 'inside': {'any': [{'kind': 'token_tree'}, {'kind': 'token_repetition'}]}}


FN = {'any': [{'kind': 'function_item'}, {'kind': 'function_signature_item'}]}
BRACES = {'kind': 'token_tree', 'regex': r'^\{'}
UNSAFE = {'has': {'kind': 'function_modifiers', 'regex': r'\bunsafe\b'}}
MUT_REF = {'kind': 'reference_type', 'has': {'kind': 'mutable_specifier'}}
STATUS = '(?:i32|i64|isize|c_int)'
C_LAYOUT = {'kind': 'token_tree', 'has': {'kind': 'identifier', 'regex': '^C$'}}
RAW_PARAM = {'kind': 'parameter', 'has': {'field': 'type', **option({'kind': 'pointer_type'})}}


def derefs(root):
    """The safe fns under `root` whose `unsafe` blocks name one of their raw-pointer parameters."""
    found = []
    for fn in root.find_all(kind='function_item', **{'not': UNSAFE}):
        # Its own parameters: a callback type's `*mut` is not a pointer the fn takes.
        names = [p.field('pattern').text() for p in fn.field('parameters').children() if p.matches(**RAW_PARAM)]
        if names and fn.find(kind='unsafe_block', has={'stopBy': 'end', **named(*map(re.escape, names))}):
            found.append(fn)
    return found


# Each count's ast-grep rule (https://ast-grep.github.io/reference/rule.html), or a fn of the tree.
KINDS = {
    'unsafe': {'any': [{'kind': 'unsafe_block'}, {**FN, **UNSAFE}, {'kind': 'impl_item', 'regex': r'^unsafe\b'},
                       tokens('unsafe', BRACES), tokens('unsafe', 'fn|impl'),
                       tokens('unsafe', 'extern', {'kind': 'string_literal'}, 'fn')]},
    'maps': named('HashMap', 'HashSet', 'BTreeMap', 'BTreeSet'),
    'lists': named('LinkedList', 'VecDeque'),
    'bools': {'any': [{'kind': 'generic_type', 'regex': r'^(?:\w+\s*::\s*)*Vec\s*<\s*bool\s*>$'},
                      {'kind': 'array_type', 'has': {'field': 'element', 'regex': '^bool$'}},
                      tokens('Vec', '<', 'bool', '>'), {'kind': 'token_tree', 'regex': r'^\[\s*bool\s*;'}]},
    'caches': {'any': [{'any': [{'kind': f'{kind}_item'} for kind in ('static', 'struct', 'enum', 'type')],
                        'has': {'field': 'name', 'regex': '(?i)cache'}},
                       tokens('static|struct|enum|type', {'kind': 'identifier', 'regex': '(?i)cache'})]},
    # A `&mut` anywhere in the parameters, even in a tuple or a callback's type, could be what
    # the fn lends, so it isn't counted.
    'mints': {'kind': 'function_item', 'all': [
        {'not': UNSAFE},
        {'has': {'field': 'return_type', **option(MUT_REF)}},
        {'not': {'has': {'field': 'parameters', 'has': {
            'stopBy': 'end', 'any': [MUT_REF, {'kind': 'self_parameter', 'regex': r'^&.*\bmut\b'}]}}}},
    ]},
    'fakes': {'any': [attribute('cfg'), {'kind': 'macro_invocation', 'has': {'field': 'macro', 'regex': '^cfg$'}}],
              'regex': r'\bnot\(\s*test\s*\)'},
    'casts': {'kind': 'identifier', 'regex': '^cast_(?:possible_truncation|sign_loss|possible_wrap|precision_loss)$'},
    'cells': {'any': [{'kind': 'generic_type', 'has': {'field': 'type', 'regex': r'(?:^|::)RefCell$'}},
                      tokens('RefCell', '<')]},
    'statuses': {'any': [{'kind': 'function_item', 'has': {'field': 'return_type', 'regex': f'^(?:\\w+::)*{STATUS}$'}},
                         tokens('->', STATUS, BRACES)]},
    'reprs': {'any': [attribute('repr', has=C_LAYOUT), tokens('repr', C_LAYOUT)]},
    'derefs': derefs,
}
# What `forbid(unsafe_code)` rejects and KINDS['unsafe'] doesn't count.
UNCOUNTED = {'any': [{'kind': 'foreign_mod_item', 'regex': r'^unsafe\b'}, attribute('unsafe')]}
FORBID = re.compile(r'^#!\[forbid\([^)]*\bunsafe_code\b', re.M)
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
C_SUFFIXES = ('.c', '.h')


def parse(path):
    """The syntax tree of the Rust file at `path`."""
    return SgRoot(path.read_text(errors='replace'), 'rust').root()


def count_file(path, test_files):
    """{kind: matches} in `path` outside #[cfg(test)] modules; adds files those modules load to `test_files`."""
    root = parse(path)
    skipped = []
    for module in root.find_all(kind='mod_item'):
        attributes, node = [], module.prev()
        while node and node.kind() in ('attribute_item', 'line_comment', 'block_comment'):
            attributes.append(node.text())
            start, node = node.range().start.index, node.prev()
        if '#[cfg(test)]' not in attributes:
            continue
        # From its first attribute: a lint the module expects is the module's.
        skipped.append(range(start, module.range().end.index))
        if not module.field('body'):
            name = module.field('name').text()
            given = next((m.group(1) for a in attributes if (m := PATH_ATTR.match(a))), None)
            base = path.parent if given or path.name in ('lib.rs', 'main.rs', 'mod.rs') else path.parent / path.stem
            test_files.add((base / (given or name + '.rs')).resolve())
            test_files.add((base / name / 'mod.rs').resolve())
    return {kind: sum(not any(node.range().start.index in span for span in skipped)
                      for node in (rule(root) if callable(rule) else root.find_all(**rule)))
            for kind, rule in KINDS.items()}


def count_crate(crate):
    """{kind: matches} over the crate's non-test Rust sources."""
    files = sorted(p for p in crate.rglob('*.rs')
                   if not {'tests', 'benches', 'target'} & set(p.relative_to(crate).parts[:-1]))
    test_files, per_file = set(), {}
    for path in files:
        per_file[path.resolve()] = count_file(path, test_files)
    return {
        **{kind: sum(n[kind] for path, n in per_file.items() if path not in test_files) for kind in KINDS},
        'clines': sum(len(p.read_text(errors='replace').splitlines())
                      for p in (crate / 'src').rglob('*') if p.suffix in C_SUFFIXES),
        'unforbidden': 0 if FORBID.search((crate / 'src/lib.rs').read_text()) else 1,
    }


def counts(root=ROOT):
    """{kind: {crate: matches}} over crates/*/, a process per core."""
    crates = sorted(p for p in (root / 'crates').iterdir() if (p / 'Cargo.toml').exists())
    with ProcessPoolExecutor() as pool:
        counted = dict(zip(crates, pool.map(count_crate, crates)))
    return {kind: {crate.name: n[kind] for crate, n in counted.items()} for kind in [*KINDS, 'clines', 'unforbidden']}


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
            and not any(parse(p).find(**UNCOUNTED) for p in (root / 'crates' / c).rglob('*.rs'))}


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
