#!/usr/bin/env python3
"""Compile unchanged and hybrid backend translation units, where the hybrid keeps them, as C++14.

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
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import hybrid_tree
from tools.build_upstream import COMMIT as PIN
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--source', type=Path, required=True)
p.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
p.add_argument('--report', type=Path, required=True)
a = p.parse_args()
if a.report.exists():
    raise SystemExit('Refusing to replace an existing compile receipt')
hybrid_source, hybrid_flags = hybrid_tree(a.archive)
inputs = ['include', 'src/reactor', 'src/network/socket.cc', 'thirdparty']
rows = []
with tempfile.TemporaryDirectory(prefix='swr-reactor-compile-') as tmp:
    tree = Path(tmp)
    data = subprocess.check_output(['git', '-C', str(a.source), 'archive', PIN, *inputs])
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive:
            if member.isfile():
                out = tree / member.name
                out.parent.mkdir(parents=True, exist_ok=True)
                out.write_bytes(archive.extractfile(member).read())
    linux = sys.platform.startswith('linux')
    backends = ['base', 'poll', 'epoll'] if linux else ['base', 'poll', 'kqueue']
    common = ['-std=c++14', '-O1', '-g', '-DHAVE_EPOLL' if linux else '-DHAVE_KQUEUE',
              *subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True).split()]
    trees = {'original': (tree, ['-I'+str(tree/'include'), '-I'+str(tree)]),
             'patched': (hybrid_source, [*hybrid_flags, '-I'+str(hybrid_source)])}
    for variant, (root, flags) in trees.items():
        for name in backends:
            source = root/'src/reactor'/(name+'.cc')
            if variant != 'original' and not source.exists():
                continue  # the hybrid deletes these units: the backends are Rust (#448), base.cc's rest is crates/reactor
            cmd = ['c++', *common, *flags, '-c', str(source), '-o', str(tree/(variant+'-'+name+'.o'))]
            result = subprocess.run(cmd, capture_output=True, text=True)
            passed = result.returncode == 0
            print(variant, name, result.returncode, 'PASS' if passed else 'FAIL', flush=True)
            rows.append(dict(variant=variant, backend=name, command=cmd, returncode=result.returncode, passed=passed, stderr=result.stderr))
a.report.parent.mkdir(parents=True, exist_ok=True)
a.report.write_text(json.dumps(dict(pin=PIN, hybrid_tree=str(hybrid_source), compiler=subprocess.check_output(['c++', '--version'], text=True).splitlines()[0], rows=rows), indent=2)+'\n')
raise SystemExit(any(not row['passed'] for row in rows))
