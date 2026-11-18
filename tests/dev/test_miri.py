"""miri.yml runs every crate with unsafe code, or this file says why Miri can't (#3559)."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import budget  # noqa: E402

# Crates with unsafe code that Miri can't run, and why; the ASan run covers them.
FOREIGN = 'its tests call a C library Miri can\'t run'
EXEMPT = {
    **dict.fromkeys(['zlib', 'cares-sys', 'curl-sys',
                     'curl', 'firebird', 'oci', 'odbc', 'pgsql', 'sqlite', 'ssh2'], FOREIGN),
    'ext': 'PHP loads this cdylib; its native guard entries tail-jump to C using assembly',
    **dict.fromkeys(['zend', 'phpcoroutines', 'phphttp', 'phpprocess', 'phpruntime',
                     'phpstdext', 'phpswoole'],
                    "PHP's Zend engine, which exists only in a loaded extension"),
}


class Miri(unittest.TestCase):
    def test_every_crate_with_unsafe_runs_under_miri_or_says_why_not(self):
        workflow = (ROOT / '.github/workflows/miri.yml').read_text()
        packages = set(re.findall(r'-p swoole-rs-([\w-]+)', workflow))
        unsafe = {crate for crate, n in budget.counts()['unsafe'].items() if n}
        self.assertEqual(unsafe - packages - EXEMPT.keys(), set(), 'crates with unsafe Miri never runs')
        self.assertEqual(packages & EXEMPT.keys(), set(), 'exempt crates miri.yml runs')
        self.assertEqual(EXEMPT.keys() - unsafe, set(), 'exempt crates with no unsafe left')


if __name__ == '__main__':
    unittest.main()
