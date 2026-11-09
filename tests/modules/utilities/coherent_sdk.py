"""Reconstruct private header SDKs from immutable export and frozen source receipts."""
import hashlib
import json
from pathlib import Path
import subprocess


def digest(data):
    return hashlib.sha256(data).hexdigest()


def stage(work, export, source, reference, root):
    receipt = json.loads((export / 'export.json').read_text())
    if receipt['source_ref'] != reference:
        raise ValueError('SDK export source differs from selected source')
    for name, entry in receipt['files'].items():
        data = (export / name).read_bytes()
        if len(data) != entry['bytes'] or digest(data) != entry['sha256']:
            raise ValueError('SDK export changed: ' + name)
    manifest = json.loads((export / 'build-manifest.json').read_text())
    inputs = manifest['inputs']
    if manifest['build_mode'] != 'immutable':
        raise ValueError('SDK requires immutable build')
    sdk = work / 'include'
    sdk.mkdir(parents=True)
    # The upstream build patches no header: its SDK is the pin's include/.
    paths = subprocess.check_output(['git', '-C', str(source), 'ls-tree', '-r', '--name-only', inputs['upstream_commit'], 'include'], text=True).splitlines()
    for path in paths:
        destination = work / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(subprocess.check_output(['git', '-C', str(source), 'show', inputs['upstream_commit'] + ':' + path]))
    for path, expected in inputs['headers_sha256'].items():
        data = subprocess.check_output(['git', '-C', str(root), 'show', reference + ':' + path])
        if digest(data) != expected:
            raise ValueError('Frozen SDK ABI differs: ' + path)
        # Production installs all declared ABI headers in one prefix include directory.
        (sdk / Path(path).name).write_bytes(data)
    (sdk / 'config.h').write_bytes((export / 'config.h').read_bytes())
    # macro_profile.py re-verifies headers_sha256 before reusing this SDK.
    return dict(string_header_sha256=digest((sdk / 'swoole_string.h').read_bytes()),
                headers_sha256={str(p): digest(p.read_bytes()) for p in sdk.rglob('*') if p.is_file()})
