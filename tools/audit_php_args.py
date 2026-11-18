#!/usr/bin/env python3
"""Compare PHP argument reads with the generated arginfo registered for each handler.

The budget counts literal argument positions passed to recognized typed accessors
and optional array extraction on a required array. --raw lists other direct value
reads for inspection: they can validate the type later. --untraced lists unresolved
bindings. Dynamic argument indices and calls through function pointers need review.
"""
import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys

from ast_grep_py import SgNode, SgRoot

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools.arginfo import CODES, MASKS  # noqa: E402

REGISTER = re.compile(r'=>\s*(?:\w+::)*(zi[mf]_\w+)\s*,\s*&(?:\w+::)*(arginfo_\w+)')
HANDLER = re.compile(r'\b(zi[mf]_\w+)\s*=>\s*((?:\w+::)*\w+)(?:::\s*<[^;]*?>)?\s*;')
TYPES = {code: ty for ty, code in CODES.items()} | {code: ty for ty, code in MASKS.items()}
READS = {'long': 'int', 'bool': 'bool', 'double': 'float', 'zstr': 'string', 'str': 'string',
         'path': 'string', 'string': 'string', 'object': 'object', 'array': 'array',
         'array_ref': 'array', 'resource': 'resource', 'zstr_or_long': 'string|int',
         'str_or_long': 'string|int'}


def arguments(call):
    return [node.text() for node in call.field('arguments').children()
            if node.is_named() and node.kind() not in ('line_comment', 'block_comment')]


def declarations(root):
    signatures = {}
    for path in sorted((root / 'crates/phpstubs/src').glob('*.rs')):
        text = path.read_text()
        tree = SgRoot(text, 'rust').root()
        for item in tree.find_all(kind='static_item'):
            params = []
            for call in item.find_all(kind='call_expression'):
                fn = call.field('function').text()
                if not fn.startswith('a::arg'):
                    continue
                args = arguments(call)
                kinds = {TYPES[code] for arg in args[3:5] for code in re.findall(r'z::(\w+)', arg)}
                if 'class' in fn:
                    kinds.add(args[3][2:-1].replace('\\\\', '\\'))
                if fn in ('a::arg_class', 'a::arg_type') and args[4] == 'true':
                    kinds.add('null')
                params.append((args[0][2:-1], kinds))
            signatures[item.field('name').text()] = params
        for original, alias in re.findall(r'pub use self::(arginfo_\w+) as (arginfo_\w+);', text):
            signatures[alias] = signatures[original]
    return signatures


@dataclass
class Body:
    path: Path
    block: SgNode
    frame: str
    position: int
    parameters: tuple


def bindings(root):
    registrations, handlers, bodies = set(), {}, {}
    for path in sorted((root / 'crates').glob('php*/src/**/*.rs')):
        source = path.read_text()
        tree = SgRoot(source, 'rust').root()
        for macro in tree.find_all(kind='macro_invocation'):
            text = macro.text()
            for comment in macro.find_all(any=[{'kind': 'line_comment'}, {'kind': 'block_comment'}]):
                text = text.replace(comment.text(), ' ')
            registrations.update(REGISTER.findall(text))
            for export, target in HANDLER.findall(text):
                handlers[export] = (path, target)
        for node in tree.find_all(kind='function_item'):
            if any(parent.kind() in ('impl_item', 'trait_item') for parent in node.ancestors()):
                continue
            params = [p for p in node.field('parameters').children() if p.kind() == 'parameter']
            for i, param in enumerate(params):
                ty = param.field('type')
                if ty and re.fullmatch(r"&(?:'\w+\s+)?(?:mut\s+)?(?:\w+::)*Call(?:<[^>]+>)?", ty.text()):
                    frame = param.field('pattern').text()
                    names = tuple(p.field('pattern').text() for p in params)
                    body = Body(path, node.field('body'), frame, i, names)
                    bodies.setdefault((path, node.field('name').text()), []).append(body)
    return registrations, handlers, bodies


def resolve(path, target, bodies):
    parts = target.split('::')
    if len(parts) == 1:
        return bodies.get((path, target), [])
    folder = path.parent
    if parts[0] == 'crate':
        folder = next(parent for parent in path.parents if parent.name == 'src')
        parts = parts[1:]
    for module in (folder.joinpath(*parts[:-1]).with_suffix('.rs'), folder.joinpath(*parts[:-1], 'mod.rs')):
        if (module, parts[-1]) in bodies:
            return bodies[module, parts[-1]]
    return []


def direct_optional_array(call):
    """call.value(n).and_then(Value::array) accepts a non-array as missing."""
    field = call.parent()
    outer = field.parent() if field and field.kind() == 'field_expression' else None
    return (outer and outer.kind() == 'call_expression' and field.field('field').text() == 'and_then'
            and arguments(outer) == ['Value::array'])


def accesses(body, bodies, visited, positions):
    """Follow forwarded call frames and literal argument indices through free functions."""
    key = body.path, body.block.range().start.index, tuple(sorted(positions.items()))
    if key in visited:
        return
    visited.add(key)
    for call in body.block.find_all(kind='call_expression'):
        fn = call.field('function')
        args = [positions.get(arg, arg) for arg in arguments(call)]
        if fn.kind() == 'field_expression' and fn.field('value').text() == body.frame:
            if args and args[0].isdigit():
                reader = fn.field('field').text()
                if reader == 'value' and direct_optional_array(call):
                    reader = 'value().and_then(Value::array)'
                yield body.path, call.range().start.line + 1, reader, int(args[0])
        elif body.frame in args and fn.kind() in ('identifier', 'scoped_identifier'):
            helpers = resolve(body.path, fn.text(), bodies)
            if len(helpers) == 1:
                helper = helpers[0]
                if helper.position < len(args) and args[helper.position] == body.frame:
                    indices = {param: arg for param, arg in zip(helper.parameters, args) if arg.isdigit()}
                    yield from accesses(helper, bodies, visited, indices)


def audit(root, raw=False):
    signatures = declarations(root)
    registrations, handlers, bodies = bindings(root)
    covered, findings = set(), set()
    for export, signature in sorted(registrations):
        if export not in handlers or signature not in signatures:
            continue
        path, target = handlers[export]
        matched = resolve(path, target, bodies)
        if len(matched) != 1:
            continue
        body = matched[0]
        covered.add((export, signature))
        for file, line, reader, n in accesses(body, bodies, set(), {}):
            params = signatures[signature]
            if not 1 <= n <= len(params):
                continue
            name, expected = params[n - 1]
            if not expected or 'mixed' in expected:
                continue
            base = reader.removesuffix('_or_null').removesuffix('_opt')
            actual = set(READS[base].split('|')) if base in READS else set()
            if actual and reader.endswith('_or_null'):
                actual.add('null')
            optional_array = reader == 'value().and_then(Value::array)' and expected == {'array'}
            if optional_array or (actual - expected) or (raw and reader in ('value', 'zval')):
                findings.add((signature, n, name, '|'.join(sorted(expected)), reader, file.relative_to(root), line))
    return covered, registrations, findings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=ROOT)
    parser.add_argument('--raw', action='store_true', help='include typed arguments accessed as raw PHP values')
    parser.add_argument('--untraced', action='store_true', help='list registrations without a resolved Rust body')
    args = parser.parse_args()
    root = args.root.resolve()
    covered, registrations, findings = audit(root, raw=args.raw)
    for row in sorted(findings):
        print(*row[:-2], f'{row[-2]}:{row[-1]}', sep='\t')
    if args.untraced:
        for export, signature in sorted(registrations - covered):
            print('untraced', signature, export, sep='\t')
    print(f'{len(covered)}/{len(registrations)} registrations traced; {len(findings)} reads for review')


if __name__ == '__main__':
    main()
