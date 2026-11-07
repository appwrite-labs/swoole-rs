"""The pinned upstream sources a gate's upstream side reads. The hybrid side reads the hybrid
build's tree (tools/build_hybrid.py's hybrid_tree), never a regenerated copy."""
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
SOURCE = ROOT / 'target/upstream/src'
sys.path.insert(0, str(ROOT))
from tools.build_upstream import COMMIT as PIN


def original(path):
    return subprocess.check_output(['git', '-C', str(SOURCE), 'show', f'{PIN}:{path}'], text=True)


# Upstream files the hybrid deleted, and the hybrid file holding what is left of each
# (swoole_config.h's macros are the generated corebase constants in the build's ABI headers).
MOVED = {'src/core/base.cc': 'src/swoole_rs_corebase_globals.cc', 'src/server/port.cc': 'src/server/master.cc', 'ext-src/swoole_server_port.cc': 'ext-src/swoole_server.cc',
         'include/swoole_config.h': '../rust-prefix/include/swoole_rs_corebase_constants.h'}


def selected(tree, path):
    """The hybrid tree's text for an upstream path, following MOVED."""
    return (Path(tree) / MOVED.get(path, path)).read_text()
