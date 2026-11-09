"""A gate's engine must be the swoole.so tools/build_ext.py recorded, with the module's bit."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.build_upstream import COMMIT
from tools.ext_identity import attest, unchanged


class ExtIdentity(unittest.TestCase):
    def setUp(self):
        folder = tempfile.TemporaryDirectory()
        self.addCleanup(folder.cleanup)
        self.manifest = Path(folder.name) / 'build-manifest.json'
        self.write(COMMIT, 'abc')

    def write(self, commit, digest):
        self.manifest.write_text(json.dumps({'extension_sha256': digest, 'upstream_commit': commit,
                                             'features': ['php'], 'profile': 'release', 'source_ref': 'ref'}))

    def test_matching_engine_is_attested_until_the_manifest_changes(self):
        receipt = attest(self.manifest, {'sha256': 'abc', 'rust_feature_bits': 4}, 4)
        self.assertEqual((receipt['extension_sha256'], receipt['source_ref']), ('abc', 'ref'))
        unchanged(self.manifest, receipt)
        self.write(COMMIT, 'def')
        with self.assertRaisesRegex(ValueError, 'changed'):
            unchanged(self.manifest, receipt)

    def test_refusals(self):
        for identity, message in [({'sha256': 'other', 'rust_feature_bits': 4}, 'not the extension'),
                                  ({'sha256': 'abc', 'rust_feature_bits': 2}, 'lacks feature bit 4')]:
            with self.assertRaisesRegex(ValueError, message):
                attest(self.manifest, identity, 4)
        self.write('another', 'abc')
        with self.assertRaisesRegex(ValueError, 'another upstream'):
            attest(self.manifest, {'sha256': 'abc', 'rust_feature_bits': 4}, 4)


if __name__ == '__main__':
    unittest.main()
