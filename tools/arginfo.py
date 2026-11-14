#!/usr/bin/env python3
"""Generate crates/phpstubs from the pinned upstream's ext-src/stubs/*.stub.php.

  arginfo.py [--source PIN]          write crates/phpstubs/src
  arginfo.py [--source PIN] --check  fail unless crates/phpstubs/src is current and
                                     the same model, rendered as gen_stub.php's C, is
                                     byte-identical to the pin's *_arginfo.h

Each stub becomes one module of `pub static arginfo_*: ArgInfo<N>` (swoole-rs-zend's
`arginfo` builders, one per gen_stub macro), so the generated headers can go. The
types, defaults, required counts and aliases follow PHP's build/gen_stub.php.
"""
import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'crates/phpstubs/src'

CODES = {'bool': '_IS_BOOL', 'int': 'IS_LONG', 'float': 'IS_DOUBLE', 'string': 'IS_STRING', 'array': 'IS_ARRAY',
         'object': 'IS_OBJECT', 'void': 'IS_VOID', 'callable': 'IS_CALLABLE', 'iterable': 'IS_ITERABLE',
         'mixed': 'IS_MIXED', 'static': 'IS_STATIC', 'never': 'IS_NEVER', 'null': 'IS_NULL', 'false': 'IS_FALSE',
         'true': 'IS_TRUE'}
MASKS = {'bool': 'MAY_BE_BOOL', 'int': 'MAY_BE_LONG', 'float': 'MAY_BE_DOUBLE', 'string': 'MAY_BE_STRING',
         'array': 'MAY_BE_ARRAY', 'object': 'MAY_BE_OBJECT', 'void': 'MAY_BE_VOID', 'callable': 'MAY_BE_CALLABLE',
         'mixed': 'MAY_BE_ANY', 'static': 'MAY_BE_STATIC', 'never': 'MAY_BE_NEVER', 'null': 'MAY_BE_NULL',
         'false': 'MAY_BE_FALSE', 'true': 'MAY_BE_TRUE'}

TOKEN = re.compile(r'''
    (?P<space>\s+) | (?P<comment>/\*.*?\*/|//[^\n]*) | (?P<attribute>\#\[[^\]]*\]) |
    (?P<directive>\#(?:ifdef|ifndef|if|else|endif)\b[^\n]*) |
    (?P<string>'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*") | (?P<variable>\$\w+) |
    (?P<name>\\?\w+(?:\\\w+)*) | (?P<punct>\.\.\.|<\?php|[(){}\[\],;:=?|&\-+*/<>!.])
''', re.S | re.X)


def tokenize(text):
    tokens, position = [], 0
    while position < len(text):
        match = TOKEN.match(text, position)
        if not match:
            raise SystemExit(f'arginfo.py: cannot tokenize at {text[position:position + 40]!r}')
        kind = match.lastgroup
        if kind not in ('space', 'attribute') and not (kind == 'comment' and not match.group().startswith('/**')):
            tokens.append((kind, match.group(), match.start(), match.end()))
        position = match.end()
    return tokens


class Stub:
    """One stub file: its functions and methods, in gen_stub's order."""

    def __init__(self, text):
        self.text, self.tokens, self.at = text, tokenize(text), 0
        self.functions, self.classes, self.conditions = [], [], []
        self.namespace = ''
        self.entries = '@generate-function-entries' in text
        self.file()

    def peek(self, offset=0):
        index = self.at + offset
        return self.tokens[index][1] if index < len(self.tokens) else None

    def take(self, expected=None):
        token = self.tokens[self.at]
        if expected is not None and token[1] != expected:
            raise SystemExit(f'arginfo.py: expected {expected!r}, found {token[1]!r}')
        self.at += 1
        return token[1]

    def skip_block(self):
        self.take('{')
        depth = 1
        while depth:
            value = self.take()
            depth += {'{': 1, '}': -1}.get(value, 0)

    def directive(self):
        line = self.take().split(None, 1)
        keyword, rest = line[0], (line[1].strip() if len(line) > 1 else '')
        if keyword == '#ifdef':
            self.conditions.append(f'defined({rest})')
        elif keyword == '#ifndef':
            self.conditions.append(f'!defined({rest})')
        elif keyword == '#if':
            self.conditions.append(rest)
        elif keyword == '#endif':
            self.conditions.pop()
        else:
            raise SystemExit(f'arginfo.py: unsupported directive {keyword}')

    def condition(self):
        return ' && '.join(self.conditions) or None

    def file(self):
        while self.at < len(self.tokens):
            self.statement(top=True)

    def statement(self, top):
        kind, value = self.tokens[self.at][0], self.peek()
        if kind == 'comment' or value in ('<?php', ';'):
            self.at += 1
        elif kind == 'directive':
            self.directive()
        elif value == 'namespace':
            self.take()
            name = ''
            if self.peek() not in ('{', ';'):
                name = self.take().lstrip('\\')
            self.namespace = name
            if self.take() == '{':
                while self.peek() != '}':
                    self.statement(top=True)
                self.take('}')
                self.namespace = ''
        elif value == 'function':
            self.functions.append(self.function(None))
        elif value in ('final', 'abstract', 'class', 'interface'):
            self.klass()
        elif value == 'const':
            while self.take() != ';':
                pass
        else:
            raise SystemExit(f'arginfo.py: unexpected {value!r}')

    def resolve(self, name):
        if name.startswith('\\'):
            return name[1:]
        return f'{self.namespace}\\{name}' if self.namespace else name

    def klass(self):
        while self.peek() in ('final', 'abstract'):
            self.take()
        self.take()  # class / interface
        name = self.resolve(self.take())
        while self.peek() != '{':
            self.take()
        self.take('{')
        methods = []
        while self.peek() != '}':
            kind = self.tokens[self.at][0]
            if kind == 'comment':
                self.take()
            elif kind == 'directive':
                self.directive()
            elif self.peek() in ('public', 'protected', 'private', 'static', 'final', 'abstract'):
                start = self.at
                while self.peek() in ('public', 'protected', 'private', 'static', 'final', 'abstract'):
                    self.take()
                if self.peek() == 'function':
                    methods.append(self.function(name))
                else:
                    self.at = start
                    while self.take() != ';':
                        pass
            elif self.peek() == 'const':
                while self.take() != ';':
                    pass
            else:
                raise SystemExit(f'arginfo.py: unexpected {self.peek()!r} in {name}')
        self.take('}')
        self.classes.append((name, methods))

    def type(self):
        if self.peek() in ('&', '...') or self.tokens[self.at][0] == 'variable':
            return None
        types = []
        nullable = self.peek() == '?'
        if nullable:
            self.take()
        while True:
            name = self.take()
            types.append(name.lower() if name.lower() in MASKS or name.lower() == 'iterable' else self.resolve(name))
            if self.peek() != '|':
                break
            self.take()
        if nullable:
            types.append('null')
        return types

    def function(self, klass):
        self.take('function')
        by_ref = self.peek() == '&'
        if by_ref:
            self.take()
        name = self.take()
        self.take('(')
        parameters = []
        while self.peek() != ')':
            types = self.type()
            send = 0
            if self.peek() == '&':
                self.take()
                send = 1
            variadic = self.peek() == '...'
            if variadic:
                self.take()
            variable = self.take()[1:]
            default = None
            if self.peek() == '=':
                self.take()
                start, depth = self.tokens[self.at][2], 0
                while depth or self.peek() not in (',', ')'):
                    value = self.take()
                    depth += {'(': 1, '[': 1, ')': -1, ']': -1}.get(value, 0)
                default = self.text[start:self.tokens[self.at - 1][3]].strip()
            parameters.append((variable, types, send, variadic, default))
            if self.peek() == ',':
                self.take()
        self.take(')')
        returns = None
        if self.peek() == ':':
            self.take()
            returns = self.type()
        if self.peek() == ';':
            self.take()
        else:
            self.skip_block()
        if klass is None:
            arginfo = 'arginfo_' + name
        else:
            arginfo = 'arginfo_class_' + klass.replace('\\', '_') + '_' + name
        return {'name': name, 'arginfo': arginfo, 'by_ref': int(by_ref), 'parameters': parameters,
                'returns': returns, 'condition': self.condition()}

    def all(self):
        return self.functions + [method for _, methods in self.classes for method in methods]


def proper(default):
    return default is not None and default != 'UNKNOWN'


def required(function):
    count = 0
    for index, (_, _, _, variadic, default) in enumerate(function['parameters']):
        if default is None and not variadic:
            count = index + 1
    return count


def builtin(name):
    return name in MASKS or name == 'iterable'


def simple(types):
    """gen_stub's tryToSimpleType: the one type besides null, if there is one."""
    rest = [name for name in types if name != 'null']
    if not rest:
        return types[0]
    return rest[0] if len(rest) == 1 else None


def mask(types):
    builtins = [name for name in types if builtin(name)]
    return '|'.join(MASKS[name] for name in builtins) if builtins else '0'


def classes(types):
    return [name for name in types if not builtin(name)]


def nullable(types):
    return 'null' in types


def shape(function):
    """What the arginfo encodes, so equal shapes share one array (gen_stub's #define)."""
    def type_key(types):
        return tuple(types) if types else None
    return (function['condition'], function['by_ref'], required(function), type_key(function['returns']),
            tuple((name, type_key(types), send, variadic, default if proper(default) else None)
                  for name, types, send, variadic, default in function['parameters']))


# gen_stub's C, for the parity check.

def c_class(name):
    escaped = name.replace('\\', '\\\\')
    for letter, code in (('u', '165'), ('U', '125'), ('N', '116')):
        escaped = escaped.replace('\\\\' + letter, '\\\\\\' + code)
    return escaped


def c_default(default):
    if not proper(default):
        return 'NULL'
    return '"' + re.sub(r'''(['"\\])''', r'\\\1', default) + '"'


def c_arginfo(function):
    name, count, by_ref, returns = function['arginfo'], required(function), function['by_ref'], function['returns']
    if returns is None:
        code = f'ZEND_BEGIN_ARG_INFO_EX({name}, 0, {by_ref}, {count})\n'
    elif (single := simple(returns)) is not None:
        if builtin(single):
            code = (f'ZEND_BEGIN_ARG_WITH_RETURN_TYPE_INFO_EX({name}, {by_ref}, {count}, {CODES[single]}, '
                    f'{int(nullable(returns))})\n')
        else:
            code = (f'ZEND_BEGIN_ARG_WITH_RETURN_OBJ_INFO_EX({name}, {by_ref}, {count}, {c_class(single)}, '
                    f'{int(nullable(returns))})\n')
    elif classes(returns):
        code = (f'ZEND_BEGIN_ARG_WITH_RETURN_OBJ_TYPE_MASK_EX({name}, {by_ref}, {count}, '
                f'{"|".join(map(c_class, classes(returns)))}, {mask(returns)})\n')
    else:
        code = f'ZEND_BEGIN_ARG_WITH_RETURN_TYPE_MASK_EX({name}, {by_ref}, {count}, {mask(returns)})\n'
    for variable, types, send, variadic, default in function['parameters']:
        kind = 'ARG_VARIADIC' if variadic else 'ARG'
        with_default = '_WITH_DEFAULT_VALUE' if proper(default) else ''
        tail = ', ' + c_default(default) if proper(default) else ''
        if types is None:
            code += f'\tZEND_{kind}_INFO{with_default}({send}, {variable}{tail})\n'
        elif (single := simple(types)) is not None:
            target = CODES[single] if builtin(single) else c_class(single)
            form = 'TYPE_INFO' if builtin(single) else 'OBJ_INFO'
            code += f'\tZEND_{kind}_{form}{with_default}({send}, {variable}, {target}, {int(nullable(types))}{tail})\n'
        elif classes(types):
            tail = '' if variadic else ', ' + c_default(default)
            code += (f'\tZEND_{kind}_OBJ_TYPE_MASK({send}, {variable}, {"|".join(map(c_class, classes(types)))}, '
                     f'{mask(types)}{tail})\n')
        else:
            code += f'\tZEND_{kind}_TYPE_MASK({send}, {variable}, {mask(types)}, {c_default(default)})\n'
    return code + 'ZEND_END_ARG_INFO()\n'


def conditioned(rows, separator=''):
    """gen_stub's generateCodeWithConditions: one #if around each run of rows under one condition."""
    code, current = '', None
    for condition, body in rows:
        if current and condition != current:
            code += '#endif\n'
            current = None
        code += separator
        if condition and condition != current:
            code += f'#if {condition}\n'
            current = condition
        code += body
    return code + ('#endif\n' if current else '')


def c_header(stub):
    rows, seen = [], []
    for function in stub.all():
        equal = next((other for other in seen if shape(other) == shape(function)), None)
        seen.append(function)
        rows.append((function['condition'],
                     f'#define {function["arginfo"]} {equal["arginfo"]}\n' if equal else c_arginfo(function)))
    code = conditioned(rows, '\n')
    if not stub.entries:
        return code + '\n'
    flat = {klass: klass.replace('\\', '_') for klass, _ in stub.classes}
    methods = [(klass, method) for klass, members in stub.classes for method in members]
    code += '\n' + conditioned([(f['condition'], f'ZEND_FUNCTION({f["name"]});\n') for f in stub.functions]
                               + [(m['condition'], f'ZEND_METHOD({flat[k]}, {m["name"]});\n') for k, m in methods])
    if stub.functions:
        code += ('\nstatic const zend_function_entry ext_functions[] = {\n'
                 + conditioned([(f['condition'], f'\tZEND_FE({f["name"]}, {f["arginfo"]})\n') for f in stub.functions])
                 + '\tZEND_FE_END\n};\n')
    for klass, members in stub.classes:
        code += (f'\nstatic const zend_function_entry class_{flat[klass]}_methods[] = {{\n'
                 + conditioned([(m['condition'], f'\tZEND_ME({flat[klass]}, {m["name"]}, {m["arginfo"]}, ZEND_ACC_PUBLIC)\n')
                                for m in members])
                 + '\tZEND_FE_END\n};\n')
    return code


# The Rust module.

def rust_string(value):
    return 'c"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'


def rust_mask(types):
    builtins = [name for name in types if builtin(name)]
    return ' | '.join('z::' + MASKS[name] for name in builtins) if builtins else '0'


def rust_default(default):
    return f'Some({rust_string(default)})' if proper(default) else 'None'


def rust_bool(value):
    return 'true' if value else 'false'


def rust_return(function):
    count, by_ref, returns = required(function), function['by_ref'], function['returns']
    if returns is None:
        return f'a::begin({count}, {by_ref})'
    single = simple(returns)
    if single is not None and builtin(single):
        return f'a::begin_type({count}, {by_ref}, z::{CODES[single]}, {rust_bool(nullable(returns))})'
    if single is not None:
        return f'a::begin_class({count}, {by_ref}, {rust_string(single)}, {rust_bool(nullable(returns))})'
    if classes(returns):
        return f'a::begin_class_mask({count}, {by_ref}, {rust_string(classes(returns)[0])}, {rust_mask(returns)})'
    return f'a::begin_mask({count}, {by_ref}, {rust_mask(returns)})'


def rust_parameter(variable, types, send, variadic, default):
    head = f'{rust_string(variable)}, {send}, {rust_bool(variadic)}'
    if types is None:
        return f'a::arg({head}, {rust_default(default)})'
    single = simple(types)
    if single is not None and builtin(single):
        return f'a::arg_type({head}, z::{CODES[single]}, {rust_bool(nullable(types))}, {rust_default(default)})'
    if single is not None:
        return (f'a::arg_class({head}, {rust_string(single)}, {rust_bool(nullable(types))}, '
                f'{rust_default(default)})')
    if classes(types):
        return (f'a::arg_class_mask({head}, {rust_string(classes(types)[0])}, {rust_mask(types)}, '
                f'{rust_default(default)})')
    return f'a::arg_mask({head}, {rust_mask(types)}, {rust_default(default)})'


def rust_module(stub, path):
    if any(len(classes(types or [])) > 1 for function in stub.all()
           for types in [function['returns']] + [p[1] for p in function['parameters']]):
        raise SystemExit(f'arginfo.py: {path.name}: a union of classes needs a zend_type_list')
    code = (f'//! Arginfo for `ext-src/stubs/{path.name}`, generated by `tools/arginfo.py`; do not edit.\n'
            '#![expect(non_upper_case_globals, reason = "gen_stub\'s arginfo names, as upstream\'s tables use them")]\n'
            'use swoole_rs_zend::arginfo as a;\n')
    body, seen = '', []
    for function in stub.all():
        equal = next((other for other in seen if shape(other) == shape(function)), None)
        seen.append(function)
        if equal:
            body += f'pub use self::{equal["arginfo"]} as {function["arginfo"]};\n'
            continue
        slots = [rust_return(function)] + [rust_parameter(*parameter) for parameter in function['parameters']]
        body += (f'pub static {function["arginfo"]}: a::ArgInfo<{len(slots)}> = a::ArgInfo([\n'
                 + ''.join(f'    {slot},\n' for slot in slots) + ']);\n')
    if 'z::' in body:
        code += 'use swoole_rs_zend::sys as z;\n'
    return code + '\n' + body


def module_name(path):
    name = path.name.removesuffix('.stub.php').removeprefix('php_swoole').lstrip('_')
    return name or 'swoole'


def rust_files(source):
    stubs = sorted((source / 'ext-src/stubs').glob('*.stub.php'))
    files = {}
    for path in stubs:
        files[module_name(path) + '.rs'] = rust_module(Stub(path.read_text()), path)
    files['lib.rs'] = (
        '//! The arginfo of every upstream stub (`ext-src/stubs/*.stub.php`) as Rust statics,\n'
        '//! generated by `tools/arginfo.py` (`--check` verifies it, and that `gen_stub.php`\'s\n'
        '//! headers say the same); the method tables in each `php*` crate point at them.\n'
        '//!\n'
        '//! Without the `php` feature the crate is empty.\n'
        '#![cfg(feature = "php")]\n'
        '#![forbid(unsafe_code)]\n\n'
        + ''.join(f'pub mod {module_name(path)};\n' for path in stubs))
    return {name: rustfmt(text) for name, text in files.items()}


def rustfmt(text):
    return subprocess.run(['rustfmt', '--edition', '2024', '--emit', 'stdout'], input=text, text=True,
                          capture_output=True, check=True).stdout


def parity(source):
    """Differences between gen_stub's headers and this model rendered as C."""
    failures = []
    for path in sorted((source / 'ext-src/stubs').glob('*.stub.php')):
        header = path.with_name(path.name.removesuffix('.stub.php') + '_arginfo.h')
        expected = header.read_text().split('\n', 2)[2]
        actual = c_header(Stub(path.read_text()))
        if actual != expected:
            failures.append(header.name)
            if '--verbose' in sys.argv:
                import difflib
                sys.stdout.writelines(difflib.unified_diff(expected.splitlines(True), actual.splitlines(True),
                                                           header.name, 'generated'))
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--source', type=Path, default=ROOT / 'target/upstream/src', help='Pinned upstream checkout')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--verbose', action='store_true', help='--check: print each header difference')
    args = parser.parse_args()
    files = rust_files(args.source)
    if not args.check:
        OUTPUT.mkdir(parents=True, exist_ok=True)
        for stale in set(p.name for p in OUTPUT.glob('*.rs')) - set(files):
            (OUTPUT / stale).unlink()
        for name, text in files.items():
            (OUTPUT / name).write_text(text)
        return 0
    failures = parity(args.source)
    current = {p.name: p.read_text() for p in OUTPUT.glob('*.rs')}
    failures += [f'crates/phpstubs/src/{name} is stale; run tools/arginfo.py'
                 for name in sorted(set(files) | set(current)) if files.get(name) != current.get(name)]
    for failure in failures:
        print(f'arginfo.py: {failure}', file=sys.stderr)
    return 1 if failures else 0


if __name__ == '__main__':
    sys.exit(main())
