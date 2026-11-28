"""Deterministic real PHP WebSocket frame outcomes, compared with the control engine.

58 original FIN/mask cases, 576 PHP string/object lifetime cases and 144 pack-only
control RSV1 compatibility cases.
"""
import functools
import json
from pathlib import Path

from tools.harness import run_isolated

CASES = 778


@functools.cache
def outcome(engine):
    result = run_isolated(engine.command(Path(__file__).with_name('differential.php')), timeout=30)
    assert result.returncode == 0, result.stderr
    value = json.loads(result.stdout)
    assert not value['errors'] and len(value['cases']) == CASES, value['errors']
    return value['cases']


def test_differential(engine, control):
    ours, theirs = outcome(engine), outcome(control)
    assert [case for case in sorted(ours.keys() | theirs.keys()) if ours.get(case) != theirs.get(case)] == []
