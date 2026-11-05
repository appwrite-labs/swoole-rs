"""A retained patch checkout must preserve another PR's newly added patch."""
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import patches


class PatchExport(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name) / 'checkout'
        self.files = Path(self.temp.name) / 'patches'
        self.work.mkdir()
        self.files.mkdir()
        patches.git(self.work, 'init', '-q')
        (self.work / 'owned').write_text('before\n')
        self.commit('pin')
        self.pin = patches.git(self.work, 'rev-parse', 'HEAD').strip()
        (self.work / 'owned').write_text('after\n')
        self.commit('0001-owned.patch')
        baseline = patches.git(self.work, 'diff', '-U1', 'HEAD~', 'HEAD')
        self.owned = self.files / '0001-owned.patch'
        # format-patch transport must stay byte-identical when its diff did not change.
        self.transport = patches.git(self.work, 'format-patch', '-1', '--stdout')
        self.owned.write_text(self.transport)
        (self.work / '.git/patches-baseline.json').write_text(json.dumps({self.owned.name: baseline}))
        self.concurrent = self.files / '99990101T000000-concurrent.patch'
        self.concurrent_text = ('diff --git a/concurrent b/concurrent\nnew file mode 100644\n'
                                '--- /dev/null\n+++ b/concurrent\n@@ -0,0 +1 @@\n+other PR\n')
        self.concurrent.write_text(self.concurrent_text)

    def commit(self, subject):
        patches.git(self.work, 'add', '-A')
        patches.git(self.work, 'commit', '-qm', subject)

    def export(self):
        with patch.object(patches, 'COMMIT', self.pin):
            patches.export(self.files, self.work)

    def test_concurrent_patch_survives_repeated_export_and_the_whole_set_applies(self):
        (self.work / 'owned').write_text('local operation\n')
        self.commit('A new local operation')
        self.export()
        first = {p.name: p.read_bytes() for p in self.files.glob('*.patch')}
        self.export()
        self.assertEqual(first, {p.name: p.read_bytes() for p in self.files.glob('*.patch')})
        self.assertEqual(self.owned.read_text(), self.transport)
        self.assertTrue(self.concurrent.is_file(), "a concurrent PR's patch was deleted")
        self.assertEqual(self.concurrent.read_text(), self.concurrent_text)
        self.assertEqual(len(first), 3)
        target = Path(self.temp.name) / 'consumer'
        subprocess.run(['git', 'clone', '-q', str(self.work), str(target)], check=True)
        patches.git(target, 'checkout', '-q', '--detach', self.pin)
        for name in sorted(first):
            patches.git(target, 'apply', str(self.files / name))
        self.assertEqual((target / 'owned').read_text(), 'local operation\n')
        self.assertEqual((target / 'concurrent').read_text(), 'other PR\n')

    def test_dropped_baseline_commit_is_removed_without_deleting_concurrent_work(self):
        patches.git(self.work, 'reset', '--hard', self.pin)
        self.export()
        self.assertFalse(self.owned.exists())
        self.assertTrue(self.concurrent.is_file(), "a concurrent PR's patch was deleted")
        self.assertEqual(self.concurrent.read_text(), self.concurrent_text)


if __name__ == '__main__':
    unittest.main()
