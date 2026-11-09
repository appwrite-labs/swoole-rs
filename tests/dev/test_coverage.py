"""tools/coverage.py: per-crate line coverage and the entry points no run reached, from llvm-cov's lcov."""
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import coverage  # noqa: E402

# As llvm-cov export -format=lcov writes a cdylib whose handlers! rows live in crates/demo but
# whose handler bodies come from the macro in crates/zend (an -Cinstrument-coverage build).
LCOV = '''SF:{root}/crates/demo/src/lib.rs
FN:4,_RNvCs1WQ06gy6Z2r_4demo3one
FN:7,swr_v1_child
FNDA:4,_RNvCs1WQ06gy6Z2r_4demo3one
FNDA:0,_RNvCs1WQ06gy6Z2r_4demo3two
FNDA:0,swr_v1_child
LF:4
LH:3
end_of_record
SF:{root}/crates/zend/src/lib.rs
FN:2,zif_one
FN:2,zif_two
FNDA:4,zif_one
FNDA:0,zif_two
FNDA:0,zif_stray
LF:1
LH:1
end_of_record
SF:/rustc/abc/library/core/src/fmt.rs
FNDA:0,swr_v1_not_ours
LF:100
LH:0
end_of_record
SF:crates/demo/src/other.rs
FNDA:0,swr_v1_child
FNDA:2,swr_v1_twice
LF:2
LH:0
end_of_record
SF:{root}/crates/demo/src/again.rs
FNDA:0,swr_v1_twice
end_of_record
'''


class Coverage(unittest.TestCase):
    def test_crates_lines_and_unreached_entry_points(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'crates/demo/src').mkdir(parents=True)
            (root / 'crates/demo/src/lib.rs').write_text('handlers! {\n    zif_one => one;\n    zif_two => two;\n}\n')
            crates, entries = coverage.parse_lcov(LCOV.format(root=root), root)
            summary = coverage.summarize(crates, entries, coverage.handler_rows(root), 'abcdef0123')
        self.assertEqual(summary['lines'], 7, 'crates/ only, relative paths too')
        self.assertEqual(summary['covered'], 4)
        self.assertEqual(summary['crates']['demo']['uncovered_entry_points'], ['swr_v1_child', 'zif_two'],
                         'a handler counts where its handlers! row is; a function seen covered once is covered')
        self.assertEqual(summary['crates']['zend']['uncovered_entry_points'], ['zif_stray'],
                         'a function no row names stays in the file llvm-cov gives')
        markdown = coverage.markdown(summary)
        self.assertIn('| demo | 6 | 50.0% | 2 |', markdown)
        self.assertIn('| **crates/** | 7 | 57.1% | 3 |', markdown)
        self.assertIn('- `zif_two`', markdown)


if __name__ == '__main__':
    unittest.main()
