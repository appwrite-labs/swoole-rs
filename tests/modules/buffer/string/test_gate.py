"""Upstream's own string.cpp assertions, compiled unchanged, on the pinned String. The hybrid
runs none of them: its String methods are Rust, and the 14 TESTs of those are Rust tests in
originals.rs (README.md); the other nine test inline helpers and `format` templates that left
the hybrid's headers."""
import json

import pytest

from string_build import build, original_ids, pinned_tree
from tools.harness import ROOT, run_isolated


def test_originals(tmp_path, pytestconfig):
    source = ROOT / 'target/upstream/src'
    if not (source / '.git').exists():
        pytest.skip('needs the pinned target/upstream/src')
    tests = pinned_tree(source, tmp_path)
    ids = original_ids(tests)
    assert len(ids) == 23
    binary = tmp_path / 'upstream'
    compiled = build(tmp_path, 'originals.cc', binary, sanitize=pytestconfig.getoption('sanitize'), extra=[tests])
    assert compiled.returncode == 0, compiled.stderr
    result = run_isolated([str(binary)], timeout=30)
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout.splitlines()[-1])
    assert outcome.get('original_ids') == ids and outcome.get('failed') == 0, outcome
