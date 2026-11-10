"""Typed queue/facade positive source gate; no Socket/network or callback probes."""
import json

import pytest

from native import LINK, build_and_run
from tools.build_ext import native_flags
from tools.harness import ROOT


@pytest.mark.parametrize('standard', ['c++14', 'c++17', 'c++20'])
def test_typed(tmp_path, archive, pytestconfig, standard):
    flags = native_flags(archive)
    command = ['c++', '-std=' + standard, '-O1', '-g', *flags,
               ROOT / 'tests/modules/buffer/typed.cc', archive, '-lpthread', *LINK[1:]]
    if pytestconfig.getoption('sanitize'):
        command += ['-fsanitize=address,undefined', '-fno-omit-frame-pointer']
    result = build_and_run(command, tmp_path / standard)
    assert result.returncode == 0, result.stderr
    json.loads(result.stdout)
