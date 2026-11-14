"""Options and fixtures shared by module gates. The first --engine is the unchanged control."""
import pytest

from tools.harness import parse_engines


def pytest_addoption(parser):
    parser.addoption('--php', default='php')
    parser.addoption('--php-extension', action='append', default=[], help='PHP module loaded before Swoole')
    parser.addoption('--engine', action='append', default=[], help='NAME=/path/to/swoole.so; repeat to compare')
    parser.addoption('--build-manifest', help="tools/build_ext.py's build-manifest.json")
    parser.addoption('--archive', help='Prebuilt Rust static archive; skips the cargo build')
    parser.addoption('--sanitize', action='store_true', help='Build native callers with ASan/UBSan')
    parser.addoption('--rust-asan', action='store_true', help='--archive was compiled with Rust ASan')


def engines(config):
    return parse_engines(config.getoption('engine'), config.getoption('php'), config.getoption('php_extension'))


def pytest_generate_tests(metafunc):
    if 'engine' in metafunc.fixturenames:
        found = engines(metafunc.config)
        metafunc.parametrize('engine', found, ids=[engine.name for engine in found])


@pytest.fixture(scope='session')
def control(request):
    found = engines(request.config)
    if not found:
        pytest.skip('needs --engine')
    return found[0]
