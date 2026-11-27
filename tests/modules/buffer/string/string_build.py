"""Build a String consumer against the pinned String sources."""
import io
import re
import shlex
import subprocess
import sys
import tarfile
from pathlib import Path

from tools.build_upstream import COMMIT
from tools.harness import run_isolated

HERE = Path(__file__).resolve().parent
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
    (tree / 'test_core.h').write_bytes(subprocess.check_output(
        ['git', '-C', str(source), 'show', COMMIT + ':core-tests/include/test_core.h']))
    tests = tree / 'string.cpp'
    tests.write_bytes(subprocess.check_output(['git', '-C', str(source), 'show', COMMIT + ':core-tests/src/core/string.cpp']))
    return tests


def original_ids(tests):
    return ['.'.join(match) for match in re.findall(r'^TEST\(\s*(\w+)\s*,\s*(\w+)\s*\)', tests.read_text(), re.MULTILINE)]


def build(tree, consumer, binary, *, sanitize=False, extra=()):
    """Compile consumer (a file under this directory) with the pinned tree's String sources.
    Returns the compiler result."""
    command = ['c++', '-std=c++17', '-O1', '-g', '-ffunction-sections', '-fdata-sections',
               '-I' + str(tree / 'include'), *map(str, extra), str(HERE / consumer),
               *[str(tree / name) for name in SOURCES]]
    command += shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'openssl', 'gtest'], text=True))
    command += ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-lm']
    if sys.platform.startswith('linux'):
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    if sanitize:
        command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
    return run_isolated([*command, '-o', str(binary)], timeout=60)
