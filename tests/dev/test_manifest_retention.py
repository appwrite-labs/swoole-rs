"""Ephemeral build provenance remains exact and source-bound after cleanup."""
import hashlib
from pathlib import Path
import tempfile
import unittest
from tools import dev, dev_report


class ManifestRetention(unittest.TestCase):
    def test_exact_manifest_survives_original_deletion_and_relocation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'ephemeral').mkdir()
            source, reports = root / 'ephemeral/build-manifest.json', root / 'reports'
            reports.mkdir()
            raw = b'{"build_mode":"immutable", "inputs":{"modules":["coroutine"]}}\n'
            source.write_bytes(raw)
            digest = hashlib.sha256(raw).hexdigest()
            build = dict(build_manifest=str(source))
            result = dev.retain_build_manifest(build, reports)
            self.assertEqual(result['sha256'], digest)
            self.assertEqual((reports / 'build-manifest.json').read_bytes(), raw)
            source.unlink()
            relocated = root / 'host-report'
            reports.rename(relocated)
            observed = dev_report.build_identity(build, relocated / 'loop.json')
            self.assertEqual(observed['manifest_sha256'], digest)
            self.assertEqual(observed['modules'], ['coroutine'])

    def test_missing_manifest_is_not_qualified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(FileNotFoundError):
                dev.retain_build_manifest(dict(build_manifest=str(root / 'missing.json')), root)
