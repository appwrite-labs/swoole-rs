"""An interrupted upstream clone never leaves a half-checked-out pin (#2495)."""
import os
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


class ReadOnlyRunner(unittest.TestCase):
    """phpize copies PHP's run-tests.php read-only; a profile lends the pin its runner (#4460)."""

    def lend(self, pinned):
        with tempfile.TemporaryDirectory() as temporary:
            source, pin = Path(temporary, 'profile/src'), Path(temporary, 'upstream/src')
            source.mkdir(parents=True)
            pin.mkdir(parents=True)
            (source / 'run-tests.php').write_text('official\n')
            (source / 'run-tests.php').chmod(0o555)
            runner = pin / 'run-tests.php'
            runner.write_text(pinned)
            runner.chmod(0o555)
            os.utime(runner, ns=(1, 1))
            before = runner.stat()
            build_upstream.runner(source, pin)
            after = runner.stat()
            self.assertEqual(runner.read_text(), 'official\n')
            self.assertEqual(after.st_mode & 0o777, 0o555)
            return before, after

    def test_an_identical_read_only_runner_is_left_untouched(self):
        before, after = self.lend('official\n')
        self.assertEqual((after.st_ino, after.st_mtime_ns), (before.st_ino, before.st_mtime_ns))

    def test_a_different_read_only_runner_is_replaced(self):
        self.lend('older php\n')


if __name__ == '__main__':
    unittest.main()
