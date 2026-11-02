"""Fail closed before any PHP execution when a reusable ZTS receipt is invalid."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ZtsOriginals(unittest.TestCase):
    def test_invalid_producer_pair_or_archive_never_runs_originals(self):
        for producer, message in [
            ({'passed': False}, 'Successful real ZTS producer required'),
            ({'passed': True, 'php': ['8.5', True], 'engines': []}, 'Exact native/selected ZTS pair required'),
            ({'passed': True, 'php': ['8.5', True], 'engines': [{'name': 'native'}, {'name': 'selected'}],
              'producer': {'archive_sha256': '0' * 64}}, 'Producer Rust archive changed'),
        ]:
            with self.subTest(message=message), tempfile.TemporaryDirectory() as directory:
                work = Path(directory)
                archive = work / 'rust-prefix/lib/libswoole_rs_php.a'
                archive.parent.mkdir(parents=True)
                archive.write_bytes(b'changed archive')
                receipt, output = work / 'producer.json', work / 'result.json'
                receipt.write_text(json.dumps({**producer, 'work': str(work)}))
                result = subprocess.run([sys.executable, str(ROOT/'tests/modules/integration/zts_originals.py'),
                    '--checkout', str(ROOT), '--source', str(work/'missing-source'),
                    '--zts-report', str(receipt), '--output', str(output)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 1, result.stderr)
                report = json.loads(output.read_text())
                self.assertFalse(report['passed'])
                self.assertFalse(report['executed'])
                self.assertEqual(report['commands'], [])
                self.assertEqual(report['error'], message)
                self.assertFalse(Path(report['work']).exists())
