"""Fixtures for the native buffer gates: the Rust archive."""
from pathlib import Path

import pytest


@pytest.fixture
def archive(pytestconfig):
    if not pytestconfig.getoption('archive'):
        pytest.skip('needs --archive')
    return Path(pytestconfig.getoption('archive')).resolve(strict=True)
