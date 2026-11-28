"""Compile and run every upstream String test against the pinned sources."""
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
    assert len(ids) == 42
    binary = tmp_path / 'upstream'
    compiled = build(tmp_path, 'originals.cc', binary, sanitize=pytestconfig.getoption('sanitize'), extra=[tests])
    assert compiled.returncode == 0, compiled.stderr
    report = tmp_path / 'results.json'
    result = run_isolated([str(binary), f'--gtest_output=json:{report}'], timeout=30)
    assert result.returncode == 0, result.stdout + result.stderr
    suites = json.loads(report.read_text())['testsuites']
    ran = [f"{suite['name']}.{case['name']}" for suite in suites for case in suite['testsuite']
           if case['status'] == 'RUN' and case['result'] == 'COMPLETED']
    assert ran == ids
