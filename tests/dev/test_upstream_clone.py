"""An interrupted upstream clone never leaves a half-checked-out pin (#2495)."""
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from tools import build_upstream


class InterruptedClone(unittest.TestCase):
    def test_killed_clone_leaves_no_pin_and_the_next_run_recovers(self):
        with tempfile.TemporaryDirectory() as temporary:
            origin, pin = Path(temporary, 'origin'), Path(temporary, 'upstream/src')
            pin.parent.mkdir()
            git = ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-C', str(origin)]
            subprocess.run(['git', 'init', '-q', str(origin)], check=True)
            (origin / 'config.m4').write_text('original\n')
            subprocess.run([*git, 'add', '.'], check=True)
            subprocess.run([*git, 'commit', '-qm', 'fixture'], check=True)
            subprocess.run([*git, 'tag', build_upstream.VERSION], check=True)

            def killed(command, **_):
                # What SIGKILL mid-checkout leaves: git metadata and part of the tree.
                (Path(command[-1]) / '.git').mkdir(parents=True)
                raise KeyboardInterrupt

            with patch.object(build_upstream.subprocess, 'run', side_effect=killed), \
                 self.assertRaises(KeyboardInterrupt):
                build_upstream.clone(pin, str(origin))
            self.assertFalse(pin.exists())

            build_upstream.clone(pin, str(origin))
            self.assertEqual((pin / 'config.m4').read_text(), 'original\n')
            self.assertFalse(pin.with_name('src.partial').exists())
            self.assertEqual(subprocess.run(['git', '-C', str(pin), 'diff', '--quiet', 'HEAD', '--']).returncode, 0)


if __name__ == '__main__':
    unittest.main()
