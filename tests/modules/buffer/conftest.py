"""Fixtures for the native buffer gates: pinned upstream sources and a Rust archive."""
import io
from pathlib import Path
import subprocess
import tarfile

import pytest

from native import SOURCE
from tools.build_upstream import COMMIT


@pytest.fixture
def pinned(tmp_path):
    """Extract pinned upstream paths into a fresh tree: pinned('include', 'src/core/buffer.cc')."""
    if not (SOURCE / '.git').exists():
        pytest.skip('needs the pinned target/upstream/src')

    def extract(*paths):
        data = subprocess.check_output(['git', '-C', str(SOURCE), 'archive', COMMIT, *paths])
        with tarfile.open(fileobj=io.BytesIO(data)) as bundle:
            for member in bundle:
                if member.isfile():
                    path = tmp_path / member.name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(bundle.extractfile(member).read())
        return tmp_path
    return extract


@pytest.fixture
def archive(pytestconfig):
    if not pytestconfig.getoption('archive'):
        pytest.skip('needs --archive')
    return Path(pytestconfig.getoption('archive')).resolve(strict=True)
