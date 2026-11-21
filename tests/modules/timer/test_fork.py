"""Isolated active-timer fork diagnostic, outside the original PHPT corpus.

An active one-shot or repeating callback forks a Swoole Process; checks child
reset/reinitialization, parent repeated callback health and cleanup.
"""
import json
import os
from pathlib import Path

import pytest

from tools.harness import run_isolated

HERE = Path(__file__).parent


def expectation(case):
    child = 'DONE'
    if case == 'child-reinit':
        child = {'inherited_missing': True, 'initial_count': 0, 'clear': True,
                 'cleared_missing': True, 'cancelled_calls': 0, 'callbacks': 2,
                 'health': 1, 'timers': 0}
    return {'callbacks': 1 if case == 'one-shot' else 3, 'health': 1,
            'child_exit': 0, 'child_signal': 0, 'child_done': child, 'timers': 0}


@pytest.mark.parametrize('case', ['one-shot', 'persistent', 'child-reinit'])
def test_active_fork(engine, case, tmp_path):
    engine.metadata()
    command = engine.command(HERE / 'fork_active.php') if case == 'one-shot' else engine.command(HERE / 'fork_persistent.php', case)
    result = run_isolated(command, timeout=10, env={**os.environ, 'TEST_DATA': str(tmp_path)})
    assert result.returncode == 0, f'diagnostic process did not exit normally: {result.stdout}{result.stderr}'
    # Native diagnostics may precede the final JSON line.
    assert json.loads(result.stdout.splitlines()[-1]) == expectation(case)
