"""tools/budget.py counts unsafe items and slow data layouts in non-test crate code and only lets them fall."""
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import budget  # noqa: E402

LIB = '''\
mod imp;
pub unsafe fn a() { unsafe { b() } }
pub unsafe extern "C" fn c() {}
unsafe impl Send for X {}
// unsafe { not code }
#[cfg(test)]
fn helper() { unsafe { counted() } }
#[cfg(test)]
mod tests {
    fn t() { unsafe { skipped() } }
}
#[cfg(test)]
#[path = "loaded_tests.rs"]
mod loaded;
use std::collections::{HashMap, VecDeque};
static DATE_CACHE: u8 = 0;
pub(crate) struct Cached;
fn flags(open: Vec<bool>, fixed: [bool; 4]) {}
'''


class Budget(unittest.TestCase):
    def test_counts_non_test_blocks_fns_and_impls(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            src = root / 'crates/one/src'
            src.mkdir(parents=True)
            (root / 'crates/one/Cargo.toml').write_text('')
            (src / 'lib.rs').write_text(LIB)
            (src / 'imp.rs').write_text('fn d() { unsafe {} }\n#[cfg(test)]\nmod tests;\n')
            (src / 'imp').mkdir()
            (src / 'imp/tests.rs').write_text('unsafe {}\n')
            (src / 'loaded_tests.rs').write_text('unsafe {}\n')
            (root / 'crates/one/tests').mkdir()
            (root / 'crates/one/tests/it.rs').write_text('unsafe {}\n')
            # a, its block, c, the impl, the cfg(test) helper's block, imp's block.
            self.assertEqual(budget.counts(root), {
                'unsafe': {'one': 6}, 'maps': {'one': 1}, 'lists': {'one': 1},
                'bools': {'one': 2}, 'caches': {'one': 2},
            })

    def test_only_a_rise_fails(self):
        above, below = budget.check({'a': 3, 'b': 1, 'new': 1}, {'a': 2, 'b': 2})
        self.assertEqual(above, {'a': (3, 2), 'new': (1, 0)})
        self.assertEqual(below, {'b': (1, 2)})


if __name__ == '__main__':
    unittest.main()
