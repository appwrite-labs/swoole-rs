"""The Cargo-built swoole.so a gate runs is the one tools/build_ext.py built: its
target/ext/build-manifest.json names the extension's checksum, the pinned upstream and the
build's features, and the engine carries the module's feature bit."""
import hashlib
import json
from pathlib import Path

from tools.build_upstream import COMMIT


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def attest(manifest_path, identity, bit):
    manifest = json.loads(Path(manifest_path).read_text())
    if manifest['upstream_commit'] != COMMIT:
        raise ValueError('Cargo build manifest names another upstream')
    if identity['sha256'] != manifest['extension_sha256']:
        raise ValueError('Engine is not the extension build_ext.py built')
    if not identity['rust_feature_bits'] & bit:
        raise ValueError(f'Engine lacks feature bit {bit}')
    return {'manifest': str(manifest_path), 'manifest_sha256': sha(manifest_path),
            'extension_sha256': identity['sha256'], 'features': manifest['features'],
            'source_ref': manifest['source_ref']}


def unchanged(manifest_path, receipt):
    """Raise if the manifest changed since `attest` returned `receipt`."""
    if sha(manifest_path) != receipt['manifest_sha256']:
        raise ValueError('Cargo build manifest changed during the gate')
