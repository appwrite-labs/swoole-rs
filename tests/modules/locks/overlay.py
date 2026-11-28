"""The pinned upstream sources a gate's upstream side reads. A hybrid side compiles with
tools/build_ext.py's native_flags, never a regenerated copy."""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'target/upstream/src'
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


def original(path):
    return subprocess.check_output(['git', '-C', str(SOURCE), 'show', f'{PIN}:{path}'], text=True)
