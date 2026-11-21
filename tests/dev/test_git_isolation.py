"""Tests that create scratch repositories never touch a repository named by inherited git variables."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import tools.harness  # noqa: F401 -- clears inherited git variables before this test's own git init

ROOT = Path(__file__).resolve().parents[2]


class GitIsolation(unittest.TestCase):
    def test_inherited_git_dir_is_left_untouched(self):
        with tempfile.TemporaryDirectory() as temp:
            sentinel = Path(temp) / 'sentinel'
            subprocess.run(['git', 'init', '-q', str(sentinel)], check=True)
            git_dir = sentinel / '.git'
            before = {path: path.read_bytes() for path in git_dir.rglob('*') if path.is_file()}
            env = {**os.environ, 'GIT_DIR': str(git_dir), 'GIT_INDEX_FILE': str(git_dir / 'index')}
            for module in ['test_frozen_source.py']:
                result = subprocess.run([sys.executable, '-m', 'unittest', 'discover', '-s', 'tests/dev', '-p', module],
                                        cwd=ROOT, env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr[-2000:])
            after = {path: path.read_bytes() for path in git_dir.rglob('*') if path.is_file()}
            self.assertEqual(before, after)


if __name__ == '__main__':
    unittest.main()
