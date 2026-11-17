"""Frame::unpack leaves its argument, and strings sharing it, as packed."""
import json
from pathlib import Path

from tools.harness import run_isolated


def test_unpack_reads_its_argument(engine):
    result = run_isolated(engine.command(Path(__file__).with_name('unpack.php')), timeout=30)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == []
