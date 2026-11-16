"""The timer benchmark rejects unverified or unoptimized Cargo artifacts."""
import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

spec = importlib.util.spec_from_file_location('timer_benchmark', Path(__file__).with_name('benchmark.py'))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


@pytest.fixture
def artifact(tmp_path):
    extension = tmp_path / 'swoole.so'
    extension.write_bytes(b'timer benchmark artifact')
    manifest = {'extension_sha256': hashlib.sha256(extension.read_bytes()).hexdigest(),
                'upstream_commit': benchmark.COMMIT, 'profile': 'release-lto',
                'source_ref': 'a' * 40, 'features': ['php']}
    return extension, manifest


def test_cargo_artifact_without_a_makefile(artifact):
    extension, manifest = artifact
    (extension.parent / 'build-manifest.json').write_text(json.dumps(manifest))
    assert benchmark.cargo_build(extension) == manifest


@pytest.mark.parametrize('field,value,message', [
    ('extension_sha256', 'different', 'hash differs'),
    ('upstream_commit', 'different', 'source pin'),
    ('profile', 'dev', 'optimized Cargo'),
    ('source_ref', '', 'source commit'),
])
def test_unverified_artifacts_are_rejected(artifact, field, value, message):
    extension, manifest = artifact
    manifest[field] = value
    (extension.parent / 'build-manifest.json').write_text(json.dumps(manifest))
    with pytest.raises(ValueError, match=message):
        benchmark.cargo_build(extension)


def test_missing_manifest_is_rejected(artifact):
    extension, _ = artifact
    with pytest.raises(FileNotFoundError):
        benchmark.cargo_build(extension)
