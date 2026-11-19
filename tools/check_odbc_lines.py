#!/usr/bin/env python3
"""Check PDO connection diagnostic locations against the pinned Swoole driver."""
import argparse
from pathlib import Path
import re
import subprocess

from build_upstream import COMMIT, ROOT


def check(pin):
    driver = (ROOT / 'native/hooks/odbc_driver.c').read_text()
    for minor in (82, 83, 84, 85):
        path = f'thirdparty/php{minor}/pdo_odbc/odbc_driver.c'
        source = subprocess.check_output(['git', '-C', pin, 'show', f'{COMMIT}:{path}'], text=True)
        block = driver.split(f'driver_file[] = "{path}";')[1].split('#undef SWR_ODBC_LINE')[0]
        recorded = []
        names = []
        for table in ('prepare_lines', 'doer_lines', 'begin_line', 'commit_lines', 'rollback_lines',
                      'attribute_line', 'setup_lines', 'connect_line'):
            if table == 'attribute_line':
                # Compilers can use either end of a multiline macro invocation for __LINE__.
                call = re.search(r'#line (\d+)\n(\s*return SWR_ODBC_LINE\([^)]*\));', block)
                start = int(call[1])
                recorded.append((start, start + call[2].count('\n')))
                names.append(table)
            else:
                values = re.search(rf'const int {table}(?:\[\])? = ([^;]+);', block)[1]
                for n in re.findall(r'\d+', values):
                    recorded.append((int(n), int(n)))
                    names.append(table)
        original = []
        for call in re.finditer(r'^\s*pdo_odbc_(?:drv|doer|stmt)_error\(([^;]+)\);', source, re.MULTILINE):
            start = source.count('\n', 0, call.start(1)) + 1
            original.append((start, start + call[1].count('\n')))
        if len(original) != len(recorded):
            raise SystemExit(f'{path}: {len(original)} error calls, {len(recorded)} recorded locations')
        for name, expected, actual in zip(names, original, recorded):
            if expected != actual:
                raise SystemExit(f'{path}: {name} reports {actual}, expected {expected}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('pin', nargs='?', type=Path, default=ROOT / 'target/upstream/src')
    check(parser.parse_args().pin)
