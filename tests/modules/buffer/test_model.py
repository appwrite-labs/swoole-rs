"""Independent reference queue against the real C ABI, plus allocation-failure replay.

The published archive runs normally; a separate fixture recompiles the unchanged
production Rust source with a test-only one-shot failing GlobalAlloc, replaying each
baseline allocation ordinal. Required storage reports NOMEM; best-effort cache
admission may recover while the same model checks payload and ownership. Fixed
seeds, bounded lengths, one owning thread;
sanitizers cover the C++ side only.
"""
import hashlib
import json
import os
from pathlib import Path
import shlex
import sys

import pytest

from native import build_and_run, untyped_hybrid
from tools.build_upstream import COMMIT
from tools.harness import ROOT, run_compiler, run_isolated

RUST_SOURCE = ROOT / 'crates/buffer/src/lib.rs'


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def selected_archive(pytestconfig):
    if pytestconfig.getoption('archive'):
        return Path(pytestconfig.getoption('archive')).resolve(strict=True)
    if not pytestconfig.getoption('manifest'):
        pytest.skip('needs --manifest or --archive')
    manifest = json.loads(Path(pytestconfig.getoption('manifest')).read_text())
    assert manifest['inputs']['upstream_commit'] == COMMIT and manifest['migrated_feature_bits'] & 2, \
        'Selected manifest must enable the buffer at the pinned upstream commit'
    prefix = next(value.partition('=')[2] for value in manifest['configure'] if value.startswith('--with-swoole-rs='))
    archive = (Path(prefix) / 'lib/libswoole_rs_ffi.a').resolve(strict=True)
    assert sha(archive) == manifest['inputs']['rust_archive_sha256'], 'Selected archive hash differs from its build manifest'
    return archive


def allocation_fixture(directory):
    """The unchanged production Rust source behind a one-shot failing GlobalAlloc."""
    rust_hash = sha(RUST_SOURCE)
    build = run_isolated(['cargo', 'build', '-p', 'swoole-rs-buffer', '-p', 'swoole-rs-corebase', '--release', '--offline', '--locked',
                          '--message-format=json'], timeout=60)
    assert build.returncode == 0, build.stderr
    artifacts = [json.loads(line) for line in build.stdout.splitlines() if line.startswith('{')]
    libraries = {row['target']['name']: Path(name) for row in artifacts
                 if row.get('reason') == 'compiler-artifact' and row['target']['name'] != 'swoole_rs_buffer'
                 for name in row['filenames'] if name.endswith('.rlib')}
    fixture = directory / 'libbuffer_model.a'
    compiled = run_compiler(['rustc', '--edition=2024', '--crate-type=staticlib', '--crate-name=swr_buffer_model',
                             '-C', 'opt-level=3', '-C', 'panic=abort',  # as the release profile builds its dependencies
                             *[arg for path in sorted({path.parent for path in libraries.values()})
                               for arg in ['-L', f'dependency={path}']],
                             *[arg for name, path in sorted(libraries.items()) for arg in ['--extern', f'{name}={path}']],
                             str(Path(__file__).with_name('model_allocator.rs')), '-o', str(fixture)], timeout=60)
    assert compiled['returncode'] == 0, compiled['stderr']
    assert sha(RUST_SOURCE) == rust_hash, 'Production Rust source changed during fixture compilation'
    return fixture


@pytest.mark.parametrize('variant', ['published', 'allocation_fixture'])
def test_model(tmp_path, pytestconfig, variant):
    archive = selected_archive(pytestconfig)
    hybrid, flags = untyped_hybrid(archive)
    library = archive if variant == 'published' else allocation_fixture(tmp_path)
    command = shlex.split(os.environ.get('CXX', 'c++')) + [
        '-std=c++17', '-O1', '-g', *flags, str(hybrid / 'src/core/buffer.cc'), str(Path(__file__).with_name('model.cc')), str(library), '-lc', '-lm']
    if variant == 'allocation_fixture':
        command += ['-DMODEL_FAILURES']
    if sys.platform.startswith('linux'):
        command += ['-ldl', '-lpthread', '-lrt', '-lutil']
    if pytestconfig.getoption('sanitize'):
        command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
    result = build_and_run(command, tmp_path / variant, timeout=90)
    assert result.returncode == 0, result.stderr
    json.loads(result.stdout)
