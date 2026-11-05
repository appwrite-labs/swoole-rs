#!/usr/bin/env python3
"""Compile unchanged and hybrid coroutine translation units.

original: the pinned tree. patched: the hybrid build's tree with its switches and
headers (hybrid_tree, #796).
"""
from pathlib import Path
import argparse
import io
import json
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_hybrid import hybrid_tree  # noqa: E402
from tools.build_upstream import COMMIT as PIN  # noqa: E402
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source', type=Path, required=True)
p.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
p.add_argument('--report', type=Path, required=True)
p.add_argument('--stack-usage', action='store_true', help='Compile the optional stack-usage callers')
p.add_argument('--timing', action='store_true', help='Compile the optional Coroutine timing callers')
p.add_argument('--standard', choices=['c++14', 'c++17'], default='c++14')
p.add_argument('--backend', choices=['header-default', 'asm', 'ucontext', 'thread'], default='header-default',
               help='Compile the requested context backend; no switching or runtime execution')
a = p.parse_args()
hybrid_source, hybrid_flags = hybrid_tree(a.archive)
units = ['base', 'channel', 'context', 'thread_context']
rows = []
with tempfile.TemporaryDirectory(prefix='swr-coroutine-compile-') as tmp:
    tree = Path(tmp)
    data = subprocess.check_output(['git', '-C', str(a.source), 'archive', PIN, 'include', 'thirdparty',
                                    *['src/coroutine/' + name + '.cc' for name in units]])
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive:
            if member.isfile():
                out = tree / member.name
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_bytes(archive.extractfile(member).read())
    common = ['-std=' + a.standard, '-O1', '-g', '-DHAVE_EPOLL' if sys.platform.startswith('linux') else '-DHAVE_KQUEUE',
              *subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True).split()]
    common += {'asm': ['-DSW_USE_ASM_CONTEXT'], 'thread': ['-DSW_USE_THREAD_CONTEXT']}.get(a.backend, [])
    common += ['-DSW_CONTEXT_DETECT_STACK_USAGE'] if a.stack_usage else []
    common += ['-DSW_CORO_TIME'] if a.timing else []
    trees = {'original': (tree, ['-I' + str(tree / 'include'), '-I' + str(tree)]),
             'patched': (hybrid_source, [*hybrid_flags, '-I' + str(hybrid_source)])}
    for variant, (root, flags) in trees.items():
        for name in units:
            cmd = ['c++', *common, *flags, '-c', str(root / ('src/coroutine/' + name + '.cc')),
                   '-o', str(tree / (variant + '-' + name + '.o'))]
            result = subprocess.run(cmd, capture_output=True, text=True)
            passed = result.returncode == 0
            print(variant, name, result.returncode, 'PASS' if passed else 'FAIL', flush=True)
            rows.append(dict(variant=variant, backend=name, command=cmd, returncode=result.returncode, passed=passed,
                             stderr=result.stderr))
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(dict(pin=PIN, hybrid_tree=str(hybrid_source), backend=a.backend, standard=a.standard,
                                    compiler=subprocess.check_output(['c++', '--version'], text=True).splitlines()[0],
                                    rows=rows), indent=2) + '\n')
raise SystemExit(any(not row['passed'] for row in rows))
