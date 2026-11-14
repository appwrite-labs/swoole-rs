"""The PHP extension build leaves the compiler environment alone without ccache."""
from pathlib import Path
from types import SimpleNamespace
import unittest

from tools import build_upstream


class PhpCompilerCache(unittest.TestCase):
    def test_disabled_preserves_compiler_environment(self):
        env = {'CC': 'custom cc', 'CFLAGS': '-O1'}
        self.assertIs(build_upstream.compiler_cache_environment(SimpleNamespace(), env, Path('build.log')), env)
        self.assertIsNone(build_upstream.compiler_cache_identity(SimpleNamespace()))


if __name__ == '__main__':
    unittest.main()
