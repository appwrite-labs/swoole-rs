"""Upstream's own string.cpp assertions, compiled unchanged, on the pinned String and on the
hybrid's, whose methods are Rust's under their upstream symbols."""
import json
import re
import subprocess

import pytest

from string_build import build, original_ids, pinned_tree
from tools.harness import ROOT, run_isolated

# These TESTs check upstream's inline string helpers, not String. Nothing in the hybrid calls
# those helpers, so they left its headers (#2089) and the TESTs run on the pinned side only.
HELPERS = ['ltrim', 'rtrim', 'strnpos', 'strnstr', 'substr_len', 'starts_with', 'ends_with']


def without_helpers(text):
    """string.cpp minus the helper TESTs, each cut from its TEST line to the next TEST."""
    for name in HELPERS:
        start = text.index('TEST(string, ' + name + ')')
        end = re.compile(r'^TEST\(', re.MULTILINE).search(text, start + 1)
        text = text[:start] + (text[end.start():] if end else '')
    return text


def test_originals(tmp_path, archive, pytestconfig):
    source = ROOT / 'target/upstream/src'
    if not (source / '.git').exists():
        pytest.skip('needs the pinned target/upstream/src')
    tests = pinned_tree(source, tmp_path)
    ids = original_ids(tests)
    assert len(ids) == 23
    hybrid_tests = tmp_path / 'string-hybrid.cpp'
    hybrid_tests.write_text(without_helpers(tests.read_text()))
    for engine in ['upstream', 'rust']:
        binary = tmp_path / engine
        engine_tests = hybrid_tests if engine == 'rust' else tests
        expected = original_ids(engine_tests)
        assert expected == (ids if engine == 'upstream' else [i for i in ids if i not in {'string.' + n for n in HELPERS}])
        compiled = build(tmp_path, engine, 'originals.cc', binary, archive, sanitize=pytestconfig.getoption('sanitize'),
                         extra=[engine_tests])
        assert compiled.returncode == 0, compiled.stderr
        if engine == 'rust':
            assert '_ZN6swoole6String6appendEPKcm' in subprocess.check_output(['nm', '-g', str(binary)], text=True)
        result = run_isolated([str(binary)], timeout=30)
        assert result.returncode == 0, result.stderr
        outcome = json.loads(result.stdout.splitlines()[-1])
        assert outcome.get('original_ids') == expected and outcome.get('failed') == 0, outcome
