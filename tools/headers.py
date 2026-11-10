#!/usr/bin/env python3
"""Check the hand-written swr_v<N>_* prototypes under native/ against cbindgen.

cbindgen runs once per crate. Every swr_v<N>_* function declared in a native header
must be exported by a crate, with the same return type, parameter types (pointer
depth, const-ness and scalar width), and parameter names, whitespace aside.
Struct, typedef and function-pointer types are named differently on the two sides
(the headers use swr_v<N>_* names, cbindgen the Rust ones), so only their pointer
shape is compared: a named type matches a function pointer or, as an enum, a 32-bit
integer, and `void *` matches any data pointer of the same depth and const-ness.
Exports that a macro generates are invisible to cbindgen; for those the check is
only that the crate's source still names the function.
"""
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Each C spelling of a scalar, by what it is.
SCALARS = {
    'void': 'void', 'bool': 'bool', 'double': 'f64', 'float': 'f32',
    'char': 'byte', 'uint8_t': 'byte', 'int8_t': 'byte', 'unsigned char': 'byte',
    'int16_t': 'i16', 'short': 'i16', 'uint16_t': 'u16', 'unsigned short': 'u16',
    'int': 'i32', 'int32_t': 'i32', 'unsigned': 'u32', 'unsigned int': 'u32', 'uint32_t': 'u32',
    'int64_t': 'i64', 'long long': 'i64', 'off_t': 'i64', 'time_t': 'i64',
    'uint64_t': 'u64', 'unsigned long long': 'u64',
    'long': 'isize', 'ssize_t': 'isize', 'intptr_t': 'isize', 'ptrdiff_t': 'isize',
    'unsigned long': 'usize', 'size_t': 'usize', 'uintptr_t': 'usize', 'socklen_t': 'u32',
    'pid_t': 'i32', 'uid_t': 'u32', 'gid_t': 'u32', 'mode_t': 'mode',
}


def strip(text):
    text = re.sub(r'/\*.*?\*/', ' ', text, flags=re.S)
    text = re.sub(r'//[^\n]*', ' ', text)
    return re.sub(r'^\s*#[^\n]*(\\\n[^\n]*)*', ' ', text, flags=re.M)


def split(params):
    depth, current, out = 0, '', []
    for ch in params:
        if ch == ',' and depth == 0:
            out.append(current)
            current = ''
            continue
        depth += ch == '('
        depth -= ch == ')'
        current += ch
    if current.strip():
        out.append(current)
    return [p.strip() for p in out]


def shape(decl):
    """(type, name) of one declaration: a scalar or 'named', with pointers and consts."""
    decl = ' '.join(decl.replace('*', ' * ').split())
    function = re.match(r'(.*?)\(\s*\*\s*(\w*)\s*\)\s*\((.*)\)$', decl)
    if function:
        return 'fn*', function.group(2)
    words = [w for w in decl.split() if w not in ('struct', 'enum', 'extern', 'SW_API', 'restrict', 'volatile')]
    name = ''
    if len(words) > 1 and re.fullmatch(r'\w+', words[-1]) and words[-1] not in SCALARS and words[-2] != 'unsigned':
        name = words.pop()
    out, base = [], []
    for word in words:
        if word == '*':
            out.append('*')
        elif word == 'const':
            out.append('const')
        else:
            base.append(word)
    base = ' '.join(base)
    kind = SCALARS.get(base, 'named')
    # `const T *` and `T const *` both qualify the pointee.
    text, consts = [kind], 0
    for token in out:
        if token == 'const':
            consts += 1
            text.append('const')
        else:
            text.append('*')
    if text[1:2] == ['const']:
        text = [kind, 'const'] + [t for t in text[2:]]
    return ' '.join(text), name


def prototypes(text):
    found = {}
    for match in re.finditer(r'([\w\s\*]*?)\b(swr_v[0-9]+_\w+)\s*\(((?:[^;{}()]|\([^;{}]*?\))*)\)\s*;', strip(text)):
        ret, name, params = match.groups()
        # A call in an inline body, not a declaration.
        if not ret.split() or set(ret.split()) & {'return', 'else', 'case', 'throw', 'new', 'delete'}:
            continue
        if re.search(r'[&.\-+"=!]', params):
            continue
        params = [] if params.strip() in ('', 'void') else split(params)
        found[name] = (shape(ret + ' x')[0], [shape(p) for p in params])
    return found


def compatible(header, rust):
    if header == rust:
        return True
    a, b = header.split(), rust.split()
    if {a[0], b[0]} <= {'named', 'fn*'} and a[1:] == b[1:] == []:
        return True
    if {a[0], b[0]} in ({'named', 'i32'}, {'named', 'u32'}) and a[1:] == b[1:] == []:
        return True
    pointee = {'void', 'byte', 'named'}
    return a[0] in pointee and b[0] in pointee and 'void' in (a[0], b[0]) and a[1:] == b[1:] and '*' in a


def sources(crate):
    return '\n'.join(path.read_text() for path in (ROOT / 'crates' / crate / 'src').rglob('*.rs'))


def exports():
    out = {}
    with tempfile.TemporaryDirectory() as tmp:
        for crate in sorted((ROOT / 'crates').iterdir()):
            if not (crate / 'Cargo.toml').exists():
                continue
            header = Path(tmp) / (crate.name + '.h')
            # Its constant-name conflicts don't touch prototypes.
            subprocess.run(['cbindgen', '--quiet', '--lang', 'c', '--output', str(header), str(crate)],
                           check=True, stderr=subprocess.DEVNULL)
            for name, proto in prototypes(header.read_text()).items():
                out[name] = (crate.name, proto)
    return out


def check(headers, exported, macro_sources):
    """Mismatches between `headers` ({path: text}) and `exported` ({name: (crate, prototype)})."""
    problems = []
    for where, text in headers.items():
        for name, (ret, params) in prototypes(text).items():
            if name not in exported:
                if not any(re.search(r'\b' + name + r'\b', text) for text in macro_sources):
                    problems.append(f'{where}: {name} is not exported by any crate')
                continue
            crate, (rust_ret, rust_params) = exported[name]
            if not compatible(ret, rust_ret):
                problems.append(f'{where}: {name} returns {ret}; crates/{crate} returns {rust_ret}')
            if len(params) != len(rust_params):
                problems.append(f'{where}: {name} takes {len(params)} parameters; crates/{crate} takes {len(rust_params)}')
                continue
            for i, ((kind, pname), (rust_kind, rust_name)) in enumerate(zip(params, rust_params)):
                if not compatible(kind, rust_kind):
                    problems.append(f'{where}: {name} parameter {i + 1} ({pname or rust_name}) is {kind}; crates/{crate} has {rust_kind}')
                elif pname and pname != rust_name:
                    problems.append(f'{where}: {name} parameter {i + 1} is named {pname}; crates/{crate} names it {rust_name}')
    return problems


if __name__ == '__main__':
    headers = {path.relative_to(ROOT): path.read_text() for path in sorted((ROOT / 'native').rglob('*.h'))}
    crates = [crate.name for crate in sorted((ROOT / 'crates').iterdir()) if (crate / 'src').is_dir()]
    problems = check(headers, exports(), [sources(crate) for crate in crates])
    for problem in problems:
        print(problem, file=sys.stderr)
    sys.exit(1 if problems else 0)
