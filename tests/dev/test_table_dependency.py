"""Coupled owners require matching implementations before build setup."""
from contextlib import nullcontext, redirect_stderr
import io
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from tools import build_hybrid, dev


class TableDependency(unittest.TestCase):
    def test_missing_companion_fails_before_lock_build_or_reference_setup(self):
        for modules, companion in [(['phpcollections'], 'memory'), (['memory'], 'phpcollections'),
                                   (['process'], 'locks'), (['http2', 'httpbindings'], 'process')]:
            with self.subTest(modules=modules):
                stderr = io.StringIO()
                flags = [arg for module in modules for arg in ['--module', module]]
                with patch('sys.argv', ['build_hybrid', *flags]), \
                     patch.object(build_hybrid, 'exclusive') as lock, patch.object(build_hybrid, 'build') as build, \
                     redirect_stderr(stderr), self.assertRaises(SystemExit) as error:
                    build_hybrid.main()
                self.assertEqual(error.exception.code, 2)
                self.assertIn('requires --module ' + companion, stderr.getvalue())
                lock.assert_not_called()
                build.assert_not_called()

    def test_thread_enabled_profile_requires_actual_coroutine_relay(self):
        stderr = io.StringIO()
        with patch('sys.argv', ['build_hybrid', '--configure=--enable-swoole-thread']), \
             patch.object(build_hybrid, 'exclusive') as lock, patch.object(build_hybrid, 'build') as build, \
             redirect_stderr(stderr), self.assertRaises(SystemExit) as error:
            build_hybrid.main()
        self.assertEqual(error.exception.code, 2)
        self.assertIn('requires --module phpcoroutines', stderr.getvalue())
        lock.assert_not_called()
        build.assert_not_called()
        with patch('sys.argv', ['build_hybrid', '--module', 'phpcoroutines',
                                '--configure=--enable-swoole-thread']), \
             patch.object(build_hybrid, 'exclusive', return_value=nullcontext()), \
             patch.object(build_hybrid, 'build', return_value=0) as build:
            self.assertEqual(build_hybrid.main(), 0)
        self.assertEqual(build.call_args.args[0].module, ['phpcoroutines'])

    def test_explicit_pair_reaches_normal_build_without_hidden_feature_addition(self):
        with patch('sys.argv', ['build_hybrid', '--module', 'phpcollections', '--module', 'memory']), \
             patch.object(build_hybrid, 'exclusive', return_value=nullcontext()), \
             patch.object(build_hybrid, 'build', return_value=0) as build:
            self.assertEqual(build_hybrid.main(), 0)
        self.assertEqual(build.call_args.args[0].module, ['phpcollections', 'memory'])

    def test_http_lane_selects_its_server_owner_and_lock_prerequisite(self):
        modules = set(dev.selected_modules(SimpleNamespace(module=['httpbindings'])))
        self.assertEqual(modules - set(dev.GRADUATED), {'httpbindings', 'http2', 'process', 'locks', 'phpcoroutines'})
