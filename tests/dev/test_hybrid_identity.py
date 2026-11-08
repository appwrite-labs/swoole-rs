import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from tools.hybrid_identity import attest
from tools.build_upstream import COMMIT


class HybridIdentity(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        files = ['crates/example/src/lib.rs', 'native/example.h', 'migration/example.patch',
                 'tools/build_hybrid.py', 'tools/build_upstream.py']
        self.hashes = {}
        for name in files:
            p = self.root / name; p.parent.mkdir(parents=True, exist_ok=True); p.write_text(name)
            self.hashes[name] = hashlib.sha256(p.read_bytes()).hexdigest()
        for command in [['git', 'init', '-q'], ['git', 'add', '.'],
                        ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture']]:
            subprocess.run(command, cwd=self.root, check=True)
        self.archive = self.root / 'selected.a'; self.archive.write_bytes(b'production archive')
        self.manifest_path = self.root / 'manifest.json'
        self.identity = {'sha256': 'extension', 'rust_feature_bits': 512}
        self.manifest = {'build_mode': 'immutable', 'extension_sha256': 'extension', 'migrated_feature_bits': 512,
            'inputs': {'upstream_commit': COMMIT,
                       'rust_archive_sha256': hashlib.sha256(self.archive.read_bytes()).hexdigest(),
                       'rust_sources_sha256': {'crates/example/src/lib.rs': self.hashes['crates/example/src/lib.rs']},
                       'headers_sha256': {'native/example.h': self.hashes['native/example.h']},
                       'patches': [{'path': 'migration/example.patch', 'sha256': self.hashes['migration/example.patch']}],
                       'builder_sha256': self.hashes['tools/build_hybrid.py'],
                       'php_builder_sha256': self.hashes['tools/build_upstream.py']},
            'rust_compilation': {'artifact': {'target': {'kind': ['staticlib']},
                                             'profile': {'opt_level': '3', 'test': False, 'debug_assertions': False}}}}

    def verify(self):
        self.manifest_path.write_text(json.dumps(self.manifest))
        return attest(self.manifest_path, self.archive, self.identity, 'HEAD', 512, root=self.root)

    def test_coherent_receipt_and_source(self):
        self.assertEqual(self.verify()['verified_inputs'], 5)

    def test_wrong_extension_and_archive_refuse_before_runtime(self):
        self.identity['sha256'] = 'other'
        with self.assertRaisesRegex(ValueError, 'extension'): self.verify()
        self.identity['sha256'] = 'extension'; self.archive.write_bytes(b'other')
        with self.assertRaisesRegex(ValueError, 'archive'): self.verify()

    def test_stale_rust_header_or_patch_source_refuses(self):
        for group in ['rust_sources_sha256', 'headers_sha256']:
            original = dict(self.manifest['inputs'][group])
            self.manifest['inputs'][group] = {next(iter(original)): 'stale'}
            with self.assertRaisesRegex(ValueError, 'Frozen source'): self.verify()
            self.manifest['inputs'][group] = original
        self.manifest['inputs']['patches'][0]['sha256'] = 'stale'
        with self.assertRaisesRegex(ValueError, 'Frozen source'): self.verify()

    def test_debug_or_missing_source_receipt_refuses(self):
        self.manifest['rust_compilation']['artifact']['profile']['debug_assertions'] = True
        with self.assertRaisesRegex(ValueError, 'production'): self.verify()
        self.manifest['rust_compilation']['artifact']['profile']['debug_assertions'] = False
        self.manifest['inputs']['rust_sources_sha256'] = {}
        with self.assertRaisesRegex(ValueError, 'Missing'): self.verify()

    def test_dev_identity_and_path_escape_refuse(self):
        self.manifest['build_mode'] = 'development'
        with self.assertRaisesRegex(ValueError, 'immutable'): self.verify()
        self.manifest['build_mode'] = 'immutable'
        self.manifest['inputs']['headers_sha256'] = {'../outside': 'fake'}
        with self.assertRaisesRegex(ValueError, 'relative path'): self.verify()
