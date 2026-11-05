"""Partial syntax coverage follows includes and notices headers in deleted patches."""
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import check_partial


class Partial(unittest.TestCase):
    def test_changed_headers_include_old_and_new_patch_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(['git', '-C', directory, '-c', 'user.name=test',
                                                '-c', 'user.email=test@example.com', *args], text=True).strip()
            git('init', '-q')
            patches = root / 'migration/patches/demo'
            patches.mkdir(parents=True)
            old = patches / 'old.patch'
            old.write_text('diff --git a/include/removed.h b/include/removed.h\n')
            git('add', '.')
            git('commit', '-qm', 'base')
            base = git('rev-parse', 'HEAD')
            old.unlink()
            (patches / 'new.patch').write_text('diff --git a/ext-src/php_swoole_private.h b/ext-src/php_swoole_private.h\n'
                                             'diff --git a/src/core/base.cc b/src/core/base.cc\n')
            git('add', '-A')
            git('commit', '-qm', 'change')
            with patch.object(check_partial, 'ROOT', root):
                self.assertEqual(check_partial.changed_headers(base), {'include/removed.h', 'ext-src/php_swoole_private.h'})

    def test_consumers_follow_transitive_includes_and_cycles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, text in {'include/shared.h': '#include "cycle.h"\n',
                               'include/cycle.h': '#include "shared.h"\n',
                               'include/wrapper.h': '#include <shared.h>\n',
                               'ext-src/caller.cc': '#include "wrapper.h"\n',
                               'src/unrelated.cc': 'int x;\n'}.items():
                file = root / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_text(text)
            self.assertEqual(check_partial.consumers(root, {'include/shared.h'},
                                                     ['ext-src/caller.cc', 'src/unrelated.cc']), ['ext-src/caller.cc'])

    @unittest.skipUnless(shutil.which('c++'), 'needs C++ compiler')
    def test_real_compiler_rejects_a_declaration_removed_from_partial_header(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'shared.h').write_text('struct Socket { bool is_udp() const { return true; } };\n')
            (root / 'caller.cc').write_text('#include "shared.h"\nbool caller(Socket *s) { return s->is_udp(); }\n')
            line = '/bin/sh libtool --tag=CXX --mode=compile c++ -std=c++14 -c caller.cc -o caller.lo -MMD -MF caller.dep -MT caller.lo'
            command = check_partial.syntax_command(line)
            self.assertEqual(subprocess.run(command, cwd=root, capture_output=True).returncode, 0)
            (root / 'shared.h').write_text('struct Socket {};\n')
            result = subprocess.run(command, cwd=root, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('is_udp', result.stderr)
            self.assertFalse((root / 'caller.lo').exists())
            self.assertFalse((root / 'caller.dep').exists())


if __name__ == '__main__':
    unittest.main()
