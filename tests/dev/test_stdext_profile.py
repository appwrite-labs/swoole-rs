"""Bind Rust's experimental VM providers to the effective native profile."""
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest
ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from tools.build_hybrid import stdext_enabled


class StdextProfile(unittest.TestCase):
    def test_explicit_native_overrides_take_precedence(self):
        cases=[(False,[],False),(True,[],True),
               (True,['--disable-swoole-stdext'],False),
               (False,['--enable-swoole-stdext'],True),
               (False,['--enable-swoole-stdext=no'],False),
               (False,['--enable-swoole-stdext=shared'],True),
               (True,['--disable-swoole-stdext','--enable-swoole-stdext=yes'],True),
               (False,['--enable-swoole-stdext','--disable-swoole-stdext'],False)]
        for requested,options,expected in cases:
            with self.subTest(requested=requested,options=options):
                self.assertEqual(stdext_enabled(SimpleNamespace(stdext=requested,configure=options)),expected)


if __name__=='__main__':unittest.main()
