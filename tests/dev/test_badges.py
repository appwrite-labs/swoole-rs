"""tools/badges.py counts the pinned upstream lines the Cargo build no longer reads, passing PHPTs and speed."""
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import badges  # noqa: E402


def write(root, files):
    for name, text in files.items():
        (root / name).parent.mkdir(parents=True, exist_ok=True)
        (root / name).write_text(text)


class Badges(unittest.TestCase):
    def test_a_pinned_file_is_kept_while_the_build_reads_its_path(self):
        with tempfile.TemporaryDirectory() as directory:
            pin, tree, native = (Path(directory) / name for name in ('pin', 'tree', 'native'))
            write(pin, {'src/a.cc': 'x\n' * 10, 'include/b.h': 'x\n' * 3, 'ext-src/c.h': 'x\n' * 2,
                        'ext-src/php_swoole_library.h': 'x\n' * 1000, 'thirdparty/lib.c': 'x\n' * 1000})
            # The build's tree still has b.h and a patched c.h, but only c.h is on the include path.
            write(tree, {'include/b.h': 'x\n', 'ext-src/c.h': 'patched\n'})
            write(native, {'include/.keep': '', 'module/unit.cc': '#include "c.h"\n#include "b.h"\n'})
            (Path(directory) / 'build.rs').write_text('')
            read = badges.built(tree, native, Path(directory) / 'build.rs')
            self.assertEqual(read, {'ext-src/c.h'})
            self.assertEqual(badges.kept(pin, read), {'src/a.cc': (0, 10), 'include/b.h': (0, 3), 'ext-src/c.h': (2, 2)})
            self.assertEqual(badges.replaced(badges.kept(pin, read)), (13, 15))

    def test_the_build_reads_the_thirdparty_units_build_rs_names_and_their_includes(self):
        with tempfile.TemporaryDirectory() as directory:
            tree, native, script = Path(directory) / 'tree', Path(directory) / 'native', Path(directory) / 'build.rs'
            script.write_text('c.file(third.join("php/x/unit.cc")).files(["api.c"].map(|f| third.join("llhttp").join(f)))\n')
            write(native, {'include/php_swoole_cxx.h': '', 'module/glue.c': '#include <ext-src/angle.h>\n'})
            write(tree, {
                # native/include comes before the tree's ext-src/; "local.h" resolves beside its includer.
                'thirdparty/php/x/unit.cc': '#include "php_swoole_cxx.h"\n#include "local.h"\n#  include <private.h>\n',
                'thirdparty/php/x/local.h': '#ifdef SW_USE_CURL\n#include "php_swoole_private.h"\n#endif\n',
                'thirdparty/php/x/private.h': '#include "ext-src/beside.h"\n',  # <...> skips the includer's directory
                'thirdparty/llhttp/api.c': '', 'thirdparty/llhttp/myapi.c': '#include "ext-src/unused.h"\n',
                'thirdparty/php/y/other.cc': '#include "ext-src/unused.h"\n',  # build.rs doesn't name it
                'ext-src/php_swoole_cxx.h': '', 'ext-src/php_swoole_private.h': '', 'ext-src/angle.h': '',
                'ext-src/private.h': '', 'ext-src/beside.h': '', 'ext-src/unused.h': ''})
            self.assertEqual(badges.built(tree, native, script), {
                'thirdparty/php/x/unit.cc', 'thirdparty/php/x/local.h', 'thirdparty/llhttp/api.c',
                'ext-src/php_swoole_private.h', 'ext-src/angle.h', 'ext-src/private.h'})

    def test_phpt_counts_cargo_passes_over_every_pinned_test(self):
        rows = [{'test': 'a', 'engine': 'upstream', 'status': 'passed'}, {'test': 'a', 'engine': 'cargo', 'status': 'passed'},
                {'test': 'b', 'engine': 'upstream', 'status': 'passed'}, {'test': 'b', 'engine': 'cargo', 'status': 'failed'},
                {'test': 'c', 'engine': 'upstream', 'status': 'skipped'}, {'test': 'c', 'engine': 'cargo', 'status': 'skipped'},
                {'test': 'd', 'engine': 'upstream', 'status': 'passed'}, {'test': 'd', 'engine': 'cargo', 'status': 'failed', 'rerun': 'passed'}]
        self.assertEqual(badges.phpt_passing(rows), (2, 4))
        # Another job's gate (a build variant) passes b; c can't apply here.
        rows.append({'test': 'b', 'engine': 'cargo', 'status': 'passed'})
        self.assertEqual(badges.phpt_passing(rows, {'c': 'darwin only'}), (3, 3))

    def test_speed_counts_workloads_faster_than_upstream_and_names_the_slowest(self):
        result = {'requests': 10, 'instructions_per_request': {
            'plaintext': {'cargo': 103.0, 'upstream': 100.0}, 'timer': {'cargo': 80.0, 'upstream': 100.0},
            'websocket': {'cargo': 120.0, 'upstream': 100.0}}}
        badge, table = badges.speed(result, {'plaintext': 1.05, 'websocket': 1.1}, 'abcdef0123')
        self.assertEqual(badge['message'], '1/3 faster · slowest websocket 1.20×')
        self.assertEqual(badge['color'], 'orange')
        self.assertIn('| websocket | 120 | 100 | 1.2000× | 1.1× | ❌ |', table)
        self.assertIn('| timer | 80 | 100 | 0.8000× |  | ✅ |', table)
        self.assertIn('`abcdef0`', table)

    def test_speed_reads_a_result_from_before_the_cargo_build_by_its_hybrid_row(self):
        result = {'instructions_per_request': {'plaintext': {'hybrid': 90.0, 'upstream': 100.0}}}
        badge, table = badges.speed(result, {}, 'abcdef0123')
        self.assertEqual(badge['message'], '1/1 faster · slowest plaintext 0.90×')
        self.assertIn('| plaintext | 90 | 100 | 0.9000× |', table)


if __name__ == '__main__':
    unittest.main()
