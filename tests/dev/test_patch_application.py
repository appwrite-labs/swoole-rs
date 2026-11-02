"""A successful Git exit is insufficient evidence that selected source changed."""
from pathlib import Path
import os
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from tools.build_hybrid import apply_patch_verified
from tools import build_hybrid

PATCH = 'diff --git a/value.cc b/value.cc\n--- a/value.cc\n+++ b/value.cc\n@@ -1 +1 @@\n-native\n+selected\n'


class VerifiedPatch(unittest.TestCase):
    def test_committed_integration_patch_destinations_match_git(self):
        folder = build_hybrid.ROOT / 'migration/patches/integration'
        for entry in build_hybrid.module_patches(folder):
            result = subprocess.run(['git', 'apply', '--numstat', str(folder / entry['path'])],
                                    cwd=build_hybrid.ROOT, text=True, capture_output=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual({line.split('\t', 2)[2] for line in result.stdout.splitlines()},
                             set(entry['upstream_paths']))

    def test_nested_ignored_stage_changes_actual_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root/'.gitignore').write_text('target/\n')
            tree = root/'target/stage'; tree.mkdir(parents=True)
            (tree/'value.cc').write_text('native\n')
            diff = root/'change.patch'; diff.write_text(PATCH)
            proof = apply_patch_verified(tree, diff, ['value.cc'])
            self.assertEqual((tree/'value.cc').read_text(), 'selected\n')
            self.assertNotEqual(proof['before'], proof['after'])
            self.assertTrue((tree/'.git').is_dir())

    def test_incremental_refresh_under_ignored_parent_changes_selected_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root / '.gitignore').write_text('target/\n')
            reference, source, staging = root / 'reference', root / 'output', root / 'target'
            for path in [reference, source, staging]:
                path.mkdir()
            subprocess.run(['git', 'init', '-q', str(reference)], check=True)
            (reference / 'value.cc').write_text('native\n')
            subprocess.run(['git', '-C', str(reference), 'add', 'value.cc'], check=True)
            subprocess.run(['git', '-C', str(reference), '-c', 'user.name=Test',
                            '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'fixture'], check=True)
            commit = subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip()
            (source / 'value.cc').write_text('native\n')
            diff = root / 'change.patch'; diff.write_text(PATCH)
            patches = [{'path': str(diff), 'upstream_paths': ['value.cc'], 'sha256': build_hybrid.digest(diff)}]
            with patch.object(build_hybrid, 'COMMIT', commit), patch.object(tempfile, 'tempdir', str(staging)), \
                    patch.dict(os.environ, {'GIT_CEILING_DIRECTORIES': ''}):
                build_hybrid.refresh_dev_sources(reference, source, [], patches)
            self.assertEqual((source / 'value.cc').read_text(), 'selected\n')

    def test_successful_noop_is_rejected_and_bad_pin_fails_before_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            tree = Path(directory); source = tree/'value.cc'; source.write_text('native\n')
            diff = tree/'change.patch'; diff.write_text(PATCH)
            with self.assertRaisesRegex(ValueError, 'changed before application'):
                apply_patch_verified(tree, diff, ['value.cc'], 'wrong-sha')
            self.assertFalse((tree/'.git').exists())
            with patch('tools.build_hybrid.subprocess.run', return_value=subprocess.CompletedProcess([], 0, '', '')):
                with self.assertRaisesRegex(ValueError, 'unchanged'):
                    apply_patch_verified(tree, diff, ['value.cc'])
            self.assertEqual(source.read_text(), 'native\n')
            with self.assertRaisesRegex(ValueError, 'safe'):
                apply_patch_verified(tree, diff, ['../escape'])


if __name__ == '__main__':
    unittest.main()
