"""Upstream's own string.cpp assertions, compiled unchanged, on the pinned String, and on the
hybrid's the TESTs whose subject is still C++ there (`format`, a template in the header)."""
import json
import re
import subprocess

import pytest

from string_build import build, original_ids, pinned_tree
from tools.harness import ROOT, run_isolated

# These TESTs check upstream's inline string helpers, not String. Nothing in the hybrid calls
# those helpers, so they left its headers (#2089) and the TESTs run on the pinned side only.
HELPERS = ['ltrim', 'rtrim', 'strnpos', 'strnstr', 'substr_len', 'starts_with', 'ends_with']
# These check String's methods, which are Rust: each is a Rust test in originals.rs (README.md).
PORTED = ['move_and_copy', 'append', 'write', 'repeat', 'release', 'ub', 'explode', 'explode_2', 'pop_1', 'pop_2',
          'reduce_1', 'reduce_2', 'reduce_3', 'append_number']


def without(text, names):
    """string.cpp minus the named TESTs, each cut from its TEST line to its closing brace."""
    for name in names:
        text, count = re.subn(r'^TEST\(string, ' + name + r'\) \{.*?^\}\n', '', text, flags=re.MULTILINE | re.DOTALL)
        assert count == 1, name
    return text


def test_originals(tmp_path, archive, pytestconfig):
    source = ROOT / 'target/upstream/src'
    if not (source / '.git').exists():
        pytest.skip('needs the pinned target/upstream/src')
    tests = pinned_tree(source, tmp_path)
    ids = original_ids(tests)
    assert len(ids) == 23
    hybrid_tests = tmp_path / 'string-hybrid.cpp'
    # The hybrid has no swoole_util.h: nothing it builds uses ScopeGuard, and neither do these TESTs.
    hybrid_tests.write_text(without(tests.read_text(), HELPERS + PORTED).replace('#include "swoole_util.h"\n', '', 1))
    for engine in ['upstream', 'rust']:
        binary = tmp_path / engine
        engine_tests = hybrid_tests if engine == 'rust' else tests
        expected = original_ids(engine_tests)
        assert expected == (ids if engine == 'upstream' else ['string.format_1', 'string.format_2'])
        compiled = build(tmp_path, engine, 'originals.cc', binary, archive, sanitize=pytestconfig.getoption('sanitize'),
                         extra=[engine_tests])
        assert compiled.returncode == 0, compiled.stderr
        if engine == 'rust':
            assert '_ZN6swoole6String19append_random_bytesEmb' in subprocess.check_output(['nm', '-g', str(binary)], text=True)
        result = run_isolated([str(binary)], timeout=30)
        assert result.returncode == 0, result.stderr
        outcome = json.loads(result.stdout.splitlines()[-1])
        assert outcome.get('original_ids') == expected and outcome.get('failed') == 0, outcome
