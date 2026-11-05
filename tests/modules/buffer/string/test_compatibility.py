"""Ordinary public String operations, original versus selected, with an exact positive
assertion count: append/write/i32/repeat/copy/termination/reduce, no POP, lifetime,
fault or custom-allocator effects."""
import json
from pathlib import Path
import shlex
import subprocess
import sys

import pytest

from tools.build_hybrid import hybrid_tree
from tools.harness import ROOT, run_compiler, run_isolated

# The hybrid deletes string.cc: Rust defines String's methods.
SOURCES = ['src/core/string.cc', 'src/core/base.cc', 'src/os/base.cc', 'src/core/base64.cc', 'src/core/log.cc',
           'src/core/error.cc']


@pytest.mark.parametrize('variant', ['original', 'selected'])
def test_compatibility(pinned, request, pytestconfig, variant):
    archive = request.getfixturevalue('archive') if variant == 'selected' else None
    work = pinned('include', *SOURCES)
    openssl = shlex.split(subprocess.check_output(['pkg-config', '--cflags', '--libs', 'openssl'], text=True))
    tree, headers = (work, ['-I' + str(work / 'include')])
    if variant == 'selected':
        tree, headers = hybrid_tree(archive)
    command = ['c++', '-std=c++17', '-O1', '-g', '-ffunction-sections', '-fdata-sections', *headers,
               *[str(tree / name) for name in SOURCES if (tree / name).exists()], str(Path(__file__).with_name('compatibility.cc')), *openssl]
    if variant == 'selected':
        command.append(str(archive))
    command += ['-Wl,-dead_strip' if sys.platform == 'darwin' else '-Wl,--gc-sections', '-lm']
    if sys.platform.startswith('linux'):
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    if pytestconfig.getoption('sanitize'):
        command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
    binary = work / variant
    compiled = run_compiler(command + ['-o', str(binary)])
    assert compiled['returncode'] == 0, compiled['stderr']
    result = run_isolated([str(binary)], timeout=30)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout.splitlines()[-1]) == {'checks': 44, 'profile': 'ordinary-no-custom-no-pop'}
    if variant == 'selected':
        for standard in ['c++14', 'c++17', 'c++20']:
            syntax = run_isolated(['c++', '-std=' + standard, '-fsyntax-only', *headers,
                                   *shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True)),
                                   str(Path(__file__).with_name('compatibility.cc')), '-O2', '-fstrict-aliasing'], timeout=30)
            assert syntax.returncode == 0, f'{standard}: {syntax.stderr}'
