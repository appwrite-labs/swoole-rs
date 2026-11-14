"""Private copies stay independent and failed publication retains old artifacts."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from tools.private_copy import copy_private


class PrivateCopy(unittest.TestCase):
    def test_independent_bytes_metadata_and_atomic_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, destination = root / 'source with spaces', root / 'output'
            source.write_bytes(b'new artifact')
            source.chmod(0o640)
            os.utime(source, ns=(1700000000123456789, 1700000000123456789))
            destination.write_bytes(b'old artifact')
            with destination.open('rb') as old_reader:
                copy_private(source, destination)
                self.assertEqual(old_reader.read(), b'old artifact')
            self.assertEqual(destination.read_bytes(), b'new artifact')
            self.assertEqual(destination.stat().st_mtime_ns, source.stat().st_mtime_ns)
            self.assertEqual(destination.stat().st_mode & 0o777, 0o640)
            with source.open('r+b') as changed:
                changed.write(b'SRC')
            self.assertEqual(destination.read_bytes(), b'new artifact')
            with destination.open('r+b') as changed:
                changed.write(b'DST')
            self.assertEqual(source.read_bytes(), b'SRC artifact')
            self.assertEqual(sorted(p.name for p in root.iterdir()), ['output', 'source with spaces'])

    def test_rejected_clone_falls_back_without_publishing_partial_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, destination = root / 'source', root / 'output'
            source.write_bytes(b'complete')
            def failed_clone(command, **_):
                Path(command[-1]).write_bytes(b'partial clone')
                return subprocess.CompletedProcess(command, 1, b'', b'clone unsupported')
            with patch('tools.private_copy.sys.platform', 'darwin'), patch(
                    'tools.private_copy.subprocess.run', side_effect=failed_clone):
                copy_private(source, destination)
            self.assertEqual(destination.read_bytes(), b'complete')
            self.assertEqual(len(list(root.iterdir())), 2)

    def test_failed_copy_keeps_previous_published_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, destination = root / 'source', root / 'output'
            source.write_bytes(b'new')
            destination.write_bytes(b'old')
            with patch('tools.private_copy.sys.platform', 'linux'), patch(
                    'tools.private_copy.shutil.copy2', side_effect=OSError('copy failed')):
                with self.assertRaises(OSError):
                    copy_private(source, destination)
            self.assertEqual(destination.read_bytes(), b'old')
            self.assertEqual(len(list(root.iterdir())), 2)
