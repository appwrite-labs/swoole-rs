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
    **dict.fromkeys(['brotli', 'zlib', 'cares-sys', 'curl-sys', 'firebird-sys', 'ssh2-sys',
                     'curl', 'firebird', 'oci', 'odbc', 'pgsql', 'sqlite', 'ssh2'], FOREIGN),
    'tls': 'OpenSSL (SSL_select_next_proto and the rest)',
    'locale': "libc's setlocale",
    'http-ffi': "locale's strncasecmp",
    'ports': 'datagram sockets: Miri opens SOCK_STREAM only',
    'oracle': 'the pinned upstream C++, for differential tests',
    **dict.fromkeys(['zend', 'phpclients', 'phpcoroutines', 'phphttp', 'phpprocess', 'phpruntime',
                     'phpserver', 'phpstdext', 'phpswoole', 'phpthread'],
                    "PHP's Zend engine, which exists only in a loaded extension"),
}


class Miri(unittest.TestCase):
    def test_every_crate_with_unsafe_runs_under_miri_or_says_why_not(self):
        workflow = (ROOT / '.github/workflows/miri.yml').read_text()
        packages = set(re.findall(r'-p swoole-rs-([\w-]+)', workflow))
        paths = set(re.findall(r'^\s*- crates/([\w-]+)/\*\*$', workflow, re.M))
        self.assertEqual(paths, packages, 'miri.yml: `paths` and the -p lists name different crates')
        unsafe = {crate for crate, n in budget.counts()['unsafe'].items() if n}
        self.assertEqual(unsafe - packages - EXEMPT.keys(), set(), 'crates with unsafe Miri never runs')
        self.assertEqual(packages & EXEMPT.keys(), set(), 'exempt crates miri.yml runs')
        self.assertEqual(EXEMPT.keys() - unsafe, set(), 'exempt crates with no unsafe left')


if __name__ == '__main__':
    unittest.main()
