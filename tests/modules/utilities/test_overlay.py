"""No compiler/runtime: current M14 staging must change declared destinations."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path[:0] = [str(ROOT), str(Path(__file__).parent)]
from overlay import apply_owned


class OverlayTests(unittest.TestCase):
    def fixture(self, root):
        root = Path(root)
        subprocess.run(['git', 'init', '-q', str(root)], check=True)
        source = root / 'nested'
        destination = source / 'src/demo.cc'
        destination.parent.mkdir(parents=True)
        destination.write_text('native\n')
        patch_path = root / 'demo.patch'
        patch_path.write_text('diff --git a/src/demo.cc b/src/demo.cc\n--- a/src/demo.cc\n+++ b/src/demo.cc\n@@ -1 +1 @@\n-native\n+selected\n')
        return source, destination, patch_path

    def test_nested_git_stages_actual_destination(self):
        with tempfile.TemporaryDirectory() as directory:
            source, destination, patch_path = self.fixture(directory)
            # Retain the real failure mechanism as an explicit negative control.
            old = subprocess.run(['git', 'apply', str(patch_path)], cwd=source, capture_output=True)
            self.assertEqual(old.returncode, 0)
            self.assertEqual(destination.read_text(), 'native\n')
            result = apply_owned(source, patch_path)
            self.assertEqual(destination.read_text(), 'selected\n')
            self.assertNotEqual(result['before'], result['after'])

    def test_successful_noop_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            source, destination, patch_path = self.fixture(directory)
            completed = subprocess.CompletedProcess([], 0, '', '')
            with patch('tools.build_hybrid.subprocess.run', return_value=completed):
                with self.assertRaisesRegex(ValueError, 'unchanged'):
                    apply_owned(source, patch_path)
            self.assertEqual(destination.read_text(), 'native\n')


if __name__ == '__main__':
    unittest.main()
