"""dev.py prune deletes target/ build output older than the window and keeps the rest."""
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

from tools import dev


class Prune(unittest.TestCase):
    def test_only_entries_older_than_the_window_go(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            old, new = time.time() - 48 * 3600, time.time()
            paths = {name: root / 'target' / name for name in ['debug', 'reports', 'upstream', 'worktrees/a1', 'worktrees/b2']}
            for path in paths.values():
                path.mkdir(parents=True)
                (path / 'file').write_bytes(b'x' * 10)
            for name, stamp in [('debug', old), ('reports', new), ('upstream', old), ('worktrees/a1', old), ('worktrees/b2', new)]:
                os.utime(paths[name], (stamp, stamp))
            os.utime(root / 'target' / 'worktrees', (old, old))
            with patch.object(dev.subprocess, 'run'):
                removed, freed = dev.prune(root, 24)
            self.assertEqual(sorted(path.relative_to(root / 'target').as_posix() for path in removed),
                             ['debug', 'worktrees/a1'])
            self.assertEqual(freed, 20)
            for name in ['reports', 'upstream', 'worktrees/b2']:
                self.assertTrue(paths[name].exists(), name)

    def test_missing_target_frees_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(dev.prune(Path(directory), 24), ([], 0))


if __name__ == '__main__':
    unittest.main()
