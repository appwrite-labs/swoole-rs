"""Channel-close resumption returns normally or propagates the waiter's bailout."""
from pathlib import Path

import pytest

from tools.harness import run_isolated


@pytest.mark.parametrize('context', ['main', 'coroutine'])
@pytest.mark.parametrize('mode', ['return', 'fatal'])
def test_channel_close_bailout(engine, control, context, mode):
    fixture = Path(__file__).with_name('close_bailout.php')
    result = run_isolated(engine.command(fixture, mode, context), timeout=10)
    expected = 'CLOSE\nRESUMED\n'
    if mode == 'return':
        expected += 'bool(true)\nAFTER\n'
    expected += 'SHUTDOWN\n'
    observed = result.returncode, result.stdout, result.stderr
    assert observed == (255 if mode == 'fatal' else 0, expected, '')
    reference = run_isolated(control.command(fixture, mode, context), timeout=10)
    assert observed == (reference.returncode, reference.stdout, reference.stderr)
