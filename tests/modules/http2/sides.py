"""Build a two-sided http2 call gate as three translation units (#796).

A gate hands over a prelude, its pieces as (side, text) pairs and a driver. Native
pieces compile against the pin; Rust pieces and the driver compile against the hybrid
build's tree. Function bodies move out of their classes, so the driver sees only
declarations and each side's code is compiled once, with that side's headers. Prelude
state must be `inline` so every TU shares one instance.
"""
import json
from pathlib import Path
import re
import subprocess

import gate

SANITIZE = ['-O1', '-fsanitize=address,undefined', '-fno-sanitize=alignment', '-fno-omit-frame-pointer']


def items(text):
    """Top-level items of a scope: directive lines, `head;` and `head{inner}` (a trailing `;` is consumed)."""
    found, pos = [], 0
    while True:
        while pos < len(text) and text[pos].isspace():
            pos += 1
        if pos == len(text):
            return found
        if text[pos] == '#':
            end = text.find('\n', pos)
            end = len(text) if end < 0 else end
            found.append(('#', text[pos:end]))
            pos = end
            continue
        brace, semi = text.find('{', pos), text.find(';', pos)
        if brace < 0 or 0 <= semi < brace:
            found.append((text[pos:semi].strip(), None))
            pos = semi + 1
            continue
        depth, end = 1, brace + 1
        while depth:
            depth += (text[end] == '{') - (text[end] == '}')
            end += 1
        found.append((text[pos:brace].strip(), text[brace + 1:end - 1]))
        pos = end
        if text[pos:].lstrip().startswith(';'):
            pos = text.index(';', pos) + 1


def split(text, owner=''):
    """(declarations, definitions) of piece text: functions are declared where they stood and
    defined out of line, qualified by `owner` (the enclosing classes)."""
    declarations, definitions = [], []
    for head, inner in items(text):
        if head == '#':
            declarations.append(inner + '\n')
            definitions.append(inner + '\n')
            continue
        function = '(' in head and not head.startswith(('namespace', 'struct', 'class', 'using'))
        if function and not owner:
            head = head.removeprefix('static ')  # one definition, linked from the driver
        if function:
            name = re.search(r'([\w~]+)\s*\($', head[:head.index('(') + 1]).start(1)
            if '=' in head[head.index('('):]:
                raise ValueError(f'Default argument in a moved function: {head}')
        if inner is None:
            declarations.append(head + ';')
        elif head.startswith('namespace'):
            inner_declarations, inner_definitions = split(inner)
            declarations.append(head + '{' + inner_declarations + '}')
            definitions.append(head + '{' + inner_definitions + '}')
        elif re.match(r'(struct|class)\s', head):
            cls = re.match(r'(?:struct|class)\s+(\w+)', head).group(1)
            inner_declarations, inner_definitions = split(inner, owner + cls + '::')
            declarations.append(head + '{' + inner_declarations + '};')
            definitions.append(inner_definitions)
        elif not function:
            declarations.append(head + '{' + inner + '};')
        elif head[:name].rstrip().endswith('::'):
            definitions.append(head + '{' + inner + '}')  # already out of line
        else:
            declarations.append(head + ';')
            definitions.append(head[:name] + owner + head[name:] + '{' + inner + '}')
    return '\n'.join(declarations), '\n'.join(definitions)


def run(work, name, prelude, pieces, driver, *, source, archive, includes, defines=()):
    """Compile, link with the archive and run. Returns (command records, the run's JSON or None)."""
    _, hybrid_flags = gate.hybrid_tree(archive)
    base = ['c++', '-std=c++17', *SANITIZE, *(f'-D{define}' for define in defines), '-I', str(gate.ROOT / 'native/http2')]
    flags = {'native': [*base, '-I', str(Path(source) / 'include')], 'rust': [*base, *hybrid_flags]}
    extra = [part for include in includes for part in ('-I', include)]
    parts = [(side, *split(text)) for side, text in pieces]
    units = {side: prelude + ''.join(d + '\n' + f + '\n' for s, d, f in parts if s == side) for side in ['native', 'rust']}
    units['driver'] = prelude + ''.join(d + '\n' for _, d, _ in parts) + driver
    commands, objects = [], []
    for unit, text in units.items():
        cpp, obj = work / f'{name}-{unit}.cpp', work / f'{name}-{unit}.o'
        cpp.write_text(text)
        commands.append([*flags[unit if unit == 'native' else 'rust'], *extra, '-c', str(cpp), '-o', str(obj)])
        objects.append(str(obj))
    executable = work / name
    commands += [['c++', *SANITIZE, *objects, str(archive), '-o', str(executable)], [str(executable)]]
    records = []
    for command in commands:
        result = subprocess.run(command, capture_output=True, text=True)
        records.append({'command': command, 'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
        if result.returncode:
            return records, None
    return records, json.loads(result.stdout)


def variants(*args, define, **kwargs):
    """run() with `define` set to 0 and to 1. Returns (command records, the completed runs' JSON)."""
    work, name, *rest = args
    commands, results = [], []
    for value in [0, 1]:
        records, result = run(work, f'{name}-{value}', *rest, defines=[f'{define}={value}'], **kwargs)
        commands += records
        results += [result] if result else []
    return commands, results
