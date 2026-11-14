#!/usr/bin/env python3
"""Add public Rust string constants to a cbindgen header as C literal macros.

cbindgen handles numeric constants and enums but does not support &str literals.
The companion view uses ASCII, JSON-compatible string escapes shared by Rust/C,
so strings remain literal macros (including sizeof and adjacent-literal callers).
"""
import json
from pathlib import Path
import re
import sys


def append_strings(source, header):
    definitions = []
    for match in re.finditer(r'pub const (\w+):\s*&str\s*=\s*("(?:\\.|[^"\\])*")\s*;', source, re.S):
        name, literal = match.groups()
        value = json.loads(literal)
        if not value.isascii():
            raise ValueError(f'{name}: native compatibility strings must be ASCII')
        definitions.append(f'#define {name} {json.dumps(value)}')
    position = header.rindex('#endif')
    return header[:position] + '\n'.join(definitions) + '\n\n' + header[position:]


if __name__ == '__main__':
    source, header = map(Path, sys.argv[1:])
    header.write_text(append_strings(source.read_text(), header.read_text()))
