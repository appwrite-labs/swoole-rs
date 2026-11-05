"""Upstream's own string.cpp assertions, compiled unchanged, on the pinned String and on the
hybrid's, whose methods are Rust's under their upstream symbols."""
import json
import subprocess

import pytest

from string_build import build, original_ids, pinned_tree
from tools.harness import ROOT, run_isolated


def test_originals(tmp_path, archive, pytestconfig):
    source = ROOT / 'target/upstream/src'
    if not (source / '.git').exists():
        pytest.skip('needs the pinned target/upstream/src')
    tests = pinned_tree(source, tmp_path)
    ids = original_ids(tests)
    assert len(ids) == 23
    for engine in ['upstream', 'rust']:
        binary = tmp_path / engine
        compiled = build(tmp_path, engine, 'originals.cc', binary, archive, sanitize=pytestconfig.getoption('sanitize'),
                         extra=[tests])
        assert compiled.returncode == 0, compiled.stderr
        if engine == 'rust':
            assert '_ZN6swoole6String6appendEPKcm' in subprocess.check_output(['nm', '-g', str(binary)], text=True)
        result = run_isolated([str(binary)], timeout=30)
        assert result.returncode == 0, result.stderr
        outcome = json.loads(result.stdout.splitlines()[-1])
        assert outcome.get('original_ids') == ids and outcome.get('failed') == 0, outcome
