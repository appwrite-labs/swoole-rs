#!/usr/bin/env python3
"""Compare every Rust MIME suffix and media type with the original pinned initializer."""
import argparse
from pathlib import Path
import re
import subprocess

from build_upstream import COMMIT, ROOT


def pairs(initializer, entry):
    pairs = []
    while initializer.strip():
        match = entry.match(initializer)
        if not match:
            raise ValueError(f'unrecognized MIME entry: {initializer[:100]!r}')
        pairs.append(match.groups())
        initializer = initializer[match.end():]
    return pairs


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pin', nargs='?', type=Path, default=ROOT / 'target/upstream/src')
    pin = parser.parse_args().pin
    cpp = subprocess.check_output(['git', '-C', pin, 'show', f'{COMMIT}:src/protocol/mime_type.cc'], text=True)
    cpp = cpp.split(' map_(', 1)[1].strip().removeprefix('{').split('});', 1)[0]
    original = pairs(re.sub(r'(?m)^\s*//[^\n]*', '', cpp),
                     re.compile(r'\s*\{\s*"([^"]+)",\s*"([^"]+)"\s*\}\s*,?'))
    rust = (ROOT / 'crates/framing/src/mime.rs').read_text().split('static CATALOG:', 1)[1]
    rust = rust.split('= &[', 1)[1].split('];', 1)[0]
    current = pairs(rust, re.compile(r'\s*\(\s*b"([^"]+)",\s*b"([^"]+)"\s*,?\s*\)\s*,?'))
    if current != original:
        raise SystemExit(f'MIME suffix/type entries differ from pinned src/protocol/mime_type.cc ({COMMIT})')
