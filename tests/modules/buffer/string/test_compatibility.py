"""Ordinary public String operations, original versus selected, with an exact positive
assertion count: append/write/i32/repeat/copy/termination/reduce, no POP, lifetime,
fault or custom-allocator effects."""
import json
from pathlib import Path
import shlex
import subprocess

import pytest

from string_build import SOURCES, build
from tools.build_ext import native_flags
from tools.harness import run_isolated


@pytest.mark.parametrize('variant', ['original', 'selected'])
def test_compatibility(pinned, request, pytestconfig, variant):
    archive = request.getfixturevalue('archive') if variant == 'selected' else None
    work = pinned('include', *SOURCES)
    binary = work / variant
    compiled = build(work, 'rust' if archive else 'upstream', 'compatibility.cc', binary, archive,
                     sanitize=pytestconfig.getoption('sanitize'))
    assert compiled.returncode == 0, compiled.stderr
    result = run_isolated([str(binary)], timeout=30)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout.splitlines()[-1]) == {'checks': 44, 'profile': 'ordinary-no-custom-no-pop'}
    if variant == 'selected':
        for standard in ['c++14', 'c++17', 'c++20']:
            syntax = run_isolated(['c++', '-std=' + standard, '-fsyntax-only', *native_flags(archive),
                                   *shlex.split(subprocess.check_output(['pkg-config', '--cflags', 'openssl'], text=True)),
                                   str(Path(__file__).with_name('compatibility.cc')), '-O2', '-fstrict-aliasing'], timeout=30)
            assert syntax.returncode == 0, f'{standard}: {syntax.stderr}'
