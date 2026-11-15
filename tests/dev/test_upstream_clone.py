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
            git = ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'tag.gpgSign=false', '-C', str(origin)]
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


def fixture_origin(origin):
    git = ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'tag.gpgSign=false', '-C', str(origin)]
    subprocess.run(['git', 'init', '-q', str(origin)], check=True)
    (origin / 'config.m4').write_text('original\n')
    subprocess.run([*git, 'add', '.'], check=True)
    subprocess.run([*git, 'commit', '-qm', 'fixture'], check=True)
    subprocess.run([*git, 'tag', build_upstream.VERSION], check=True)


class StrayTree(unittest.TestCase):
    def test_a_tree_without_git_is_replaced_by_the_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            origin, pin = Path(temporary, 'origin'), Path(temporary, 'upstream/src')
            fixture_origin(origin)
            # What a cache restore of build outputs alone leaves: files, no .git.
            (pin / 'modules').mkdir(parents=True)
            (pin / 'modules/swoole.so').write_text('stale\n')

            build_upstream.prepare_pin(pin, str(origin))
            self.assertEqual((pin / 'config.m4').read_text(), 'original\n')
            self.assertFalse((pin / 'modules').exists())
            top = subprocess.check_output(['git', '-C', str(pin), 'rev-parse', '--show-toplevel'], text=True).strip()
            self.assertEqual(Path(top).resolve(), pin.resolve())

    def test_a_checkout_is_kept(self):
        with tempfile.TemporaryDirectory() as temporary:
            pin = Path(temporary, 'upstream/src')
            subprocess.run(['git', 'init', '-q', str(pin)], check=True)
            (pin / 'kept').write_text('kept\n')
            with patch.object(build_upstream, 'clone', side_effect=AssertionError('cloned over a checkout')):
                build_upstream.prepare_pin(pin)
            self.assertEqual((pin / 'kept').read_text(), 'kept\n')


if __name__ == '__main__':
    unittest.main()
