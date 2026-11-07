"""Build a String consumer against the pinned String sources, or the hybrid build's sources
and headers with its Rust archive."""
import io
import re
import shlex
import subprocess
import sys
import tarfile
from pathlib import Path

from tools.build_hybrid import hybrid_tree, zend_free_native_sources
from tools.build_upstream import COMMIT
from tools.harness import ROOT, run_isolated

HERE = Path(__file__).resolve().parent
# The hybrid deletes string.cc (Rust defines String's methods), base.cc and misc.cc (what is left
# of them is native/corebase/globals.cc, which it stages with its other native sources).
SOURCES = ['src/core/string.cc', 'src/core/base.cc', 'src/core/misc.cc', 'src/os/base.cc', 'src/core/base64.cc', 'src/core/log.cc',
           'src/core/error.cc']


def pinned_tree(source, tree):
    """Extract the pinned String sources and the original string.cpp tests into tree."""
    data = subprocess.check_output(['git', '-C', str(source), 'archive', COMMIT, 'include', *SOURCES])
    with tarfile.open(fileobj=io.BytesIO(data)) as bundle:
        for member in bundle:
            if member.isfile():
                path = tree / member.name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bundle.extractfile(member).read())
    tests = tree / 'string.cpp'
    tests.write_bytes(subprocess.check_output(['git', '-C', str(source), 'show', COMMIT + ':core-tests/src/core/string.cpp']))
    return tests


def original_ids(tests):
    return ['.'.join(match) for match in re.findall(r'^TEST\(\s*(\w+)\s*,\s*(\w+)\s*\)', tests.read_text(), re.MULTILINE)]


def build(tree, engine, consumer, binary, archive=None, *, sanitize=False, optimize=False, extra=()):
    """Compile consumer (a file under this directory) for engine 'upstream' (the pinned tree)
    or 'rust' (the hybrid build's tree). Returns the compiler result."""
    headers = ['-I' + str(tree / 'include')]
    if engine == 'rust':
        tree, headers = hybrid_tree(archive)
    command = ['c++', '-std=c++17', '-O3' if optimize else '-O1', '-g', '-ffunction-sections', '-fdata-sections',
               '-I' + str(HERE), *headers, *[str(tree / name) for name in [*SOURCES, *zend_free_native_sources()] if (tree / name).exists()], *map(str, extra), str(HERE / consumer)]
    command += shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'openssl'], text=True))
    if engine == 'rust':
        command.append(str(archive))
    command += ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-lm']
    if sys.platform.startswith('linux'):
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    if sanitize:
        command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
    return run_isolated([*command, '-o', str(binary)], timeout=60)
