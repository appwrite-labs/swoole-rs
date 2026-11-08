"""Verify selected first-party source inputs against an immutable hybrid receipt.

Registry/std contents are outside this attestation; their lock/compiler/archive
identities remain separate. Native reference provenance is not inferred here.
"""
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
from tools.harness import ROOT
from tools.build_upstream import COMMIT


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def attest(manifest_path, archive, identity, source_ref, expected_bit, *, root=ROOT):
    manifest_path, archive = Path(manifest_path), Path(archive)
    manifest = json.loads(manifest_path.read_text())
    inputs = manifest['inputs']
    if (manifest.get('build_mode') != 'immutable' or inputs['upstream_commit'] != COMMIT
            or identity['sha256'] != manifest['extension_sha256']
            or identity['rust_feature_bits'] != manifest['migrated_feature_bits']
            or not identity['rust_feature_bits'] & expected_bit):
        raise ValueError('Selected immutable extension/configuration identity differs')
    if sha(archive) != inputs['rust_archive_sha256']:
        raise ValueError('Selected archive differs from compiled build receipt')
    artifact = manifest['rust_compilation']['artifact']
    if (artifact['target']['kind'] != ['staticlib'] or artifact['profile']['opt_level'] != '3'
            or artifact['profile']['test'] or artifact['profile']['debug_assertions']):
        raise ValueError('Actual Cargo artifact is not the production static library')
    sources = inputs['rust_sources_sha256']
    if not sources or not inputs['headers_sha256'] or not inputs['patches']:
        raise ValueError('Missing selected first-party source attestation')
    expected = dict(sources)
    expected.update(inputs['headers_sha256'])
    expected.update({item['path']: item['sha256'] for item in inputs['patches']})
    expected.update({'tools/build_hybrid.py': inputs['builder_sha256'],
                     'tools/build_upstream.py': inputs['php_builder_sha256']})
    source = subprocess.check_output(['git', 'rev-parse', '--verify', '--end-of-options',
                                      source_ref + '^{commit}'], cwd=root, text=True).strip()
    for relative, digest in expected.items():
        path = Path(relative)
        if path.is_absolute() or '..' in path.parts:
            raise ValueError('Source receipt contains an invalid relative path')
        content = subprocess.check_output(['git', 'show', source + ':' + relative], cwd=root)
        if hashlib.sha256(content).hexdigest() != digest:
            raise ValueError('Frozen source differs from compiled input: ' + relative)
    return {'record_type': 'selected_first_party_build_inputs', 'source_ref': source,
            'manifest_sha256': sha(manifest_path), 'archive_sha256': sha(archive),
            'extension_sha256': identity['sha256'], 'verified_inputs': len(expected),
            'scope': __doc__, 'helper_sha256': sha(__file__)}


def headers(source, export_path, manifest, destination):
    export = json.loads(export_path.read_text())
    def git(path):
        return subprocess.check_output(['git', 'show', export['source_ref'] + ':' + path], cwd=ROOT)
    for name in ['config.h', 'build-manifest.json', 'swoole.so']:
        if sha(export_path.parent / name) != export['files'][name]['sha256']:
            raise ValueError('Export input changed: ' + name)
    if sha(export_path.parent / 'build-manifest.json') != sha(manifest):
        raise ValueError('Export does not identify the actual manifest')
    data = json.loads(manifest.read_text())
    if data['inputs']['upstream_commit'] != COMMIT:
        raise ValueError('Native source pin differs')
    destination.mkdir()
    archive = subprocess.check_output(['git', '-C', str(source), 'archive', COMMIT, 'include'])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        for member in tar:
            path = Path(member.name)
            if not member.isfile():
                continue
            if path.is_absolute() or '..' in path.parts:
                raise ValueError('Invalid pinned header path')
            output = destination / path
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(tar.extractfile(member).read())
    # No enclosing Git checkout can silently mask these patch paths.
    subprocess.run(['git', 'init', '-q', str(destination)], check=True)
    for index, entry in enumerate(data['inputs']['patches']):
        paths = [name for name in entry['upstream_paths'] if name.startswith('include/') and Path(name).suffix in ['.h', '.hpp', '.inc']]
        if not paths:
            continue
        patch = destination / ('patch-' + str(index))
        patch.write_bytes(git(entry['path']))
        if sha(patch) != entry['sha256']:
            raise ValueError('Compiled header patch differs from exact source')
        args = ['git', 'apply', *['--include=' + name for name in paths], str(patch)]
        subprocess.run([*args[:2], '--check', *args[2:]], cwd=destination, capture_output=True, check=True)
        subprocess.run(args, cwd=destination, capture_output=True, check=True)
    shutil.copy2(export_path.parent / 'config.h', destination / 'include/config.h')
    seen = set()
    for name, expected in data['inputs']['headers_sha256'].items():
        output = destination / 'include' / Path(name).name
        if output.name in seen:
            raise ValueError('Compiled ABI header basenames collide')
        seen.add(output.name)
        output.write_bytes(git(name))
        if sha(output) != expected:
            raise ValueError('Compiled ABI header differs from exact source')
    return dict(source_ref=export['source_ref'],
                files_sha256={str(path.relative_to(destination)): sha(path) for path in sorted((destination / 'include').rglob('*')) if path.is_file()})


def immutable_export_input(extension, exports):
    """Bind one exact regular export DSO, refusing aliases before PHP is loaded."""
    extension = Path(extension).absolute()
    if extension != extension.resolve():
        raise ValueError('Borrowed extension must be a canonical export path, not an alias')
    matches = []
    for candidate in exports:
        candidate = Path(candidate).absolute()
        if candidate != candidate.resolve():
            raise ValueError('Borrowed export must be a canonical path, not an alias')
        if extension == candidate.parent / 'swoole.so':
            matches.append(candidate)
    if len(matches) != 1:
        raise ValueError('Borrowed extension must be the exact identified export DSO')
    export = matches[0]
    data = json.loads(export.read_text())
    source_ref = data['source_ref']
    if len(source_ref) != 40 or any(c not in '0123456789abcdef' for c in source_ref):
        raise ValueError('Borrowed export needs an exact source commit')
    observed = subprocess.check_output(['git', 'rev-parse', '--verify', source_ref + '^{commit}'], cwd=ROOT, text=True).strip()
    if observed != source_ref:
        raise ValueError('Borrowed export source is unavailable or differs')
    files = data['files']
    if not {'config.h', 'build-manifest.json', 'swoole.so'} <= files.keys():
        raise ValueError('Borrowed export is incomplete')
    bound = {str(export): sha(export)}
    for name, identity in files.items():
        if Path(name).name != name or name in ['.', '..']:
            raise ValueError('Invalid borrowed export member')
        path = export.parent / name
        if path != path.resolve() or not path.is_file() or sha(path) != identity['sha256']:
            raise ValueError('Borrowed export input changed: ' + name)
        bound[str(path)] = identity['sha256']
    manifest = json.loads((export.parent / 'build-manifest.json').read_text())
    if manifest['extension_sha256'] != files['swoole.so']['sha256']:
        raise ValueError('Borrowed manifest does not identify actual extension')
    return dict(mode='borrowed-owner-private-immutable-export', extension=str(extension),
                source_ref=source_ref, export=str(export), before_sha256=bound)


def verify_borrowed_input(binding):
    after = {name: sha(Path(name)) for name in binding['before_sha256']}
    binding['after_sha256'] = after
    if after != binding['before_sha256']:
        raise ValueError('Borrowed immutable export changed during actual consumer run')
