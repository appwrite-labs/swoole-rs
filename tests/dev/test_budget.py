"""tools/budget.py counts unsafe items and slow data layouts in non-test crate code; none may rise over the base commit."""
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
#[cfg(test)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "a multi-line attribute [between] cfg(test) and the module"
)]
mod spanned {
    fn t() { unsafe { skipped() } }
}
use std::collections::{HashMap, VecDeque};
static DATE_CACHE: u8 = 0;
pub(crate) struct Cached;
fn flags(open: Vec<bool>, fixed: [bool; 4]) {}
pub fn fields(self) -> &'static mut Record {}
pub(crate) fn slot(&self, index: usize) -> Option<&mut u8> {}
fn edit(&mut self) -> &mut u8 {}
fn pick<'a>(slots: &'a mut [u8]) -> &'a mut u8 {}
fn with(field: fn(&mut Record) -> &mut u8) {}
fn unit(_: ()) -> &'static mut u8 {}
fn paired(pair: (u8, u8)) -> &'static mut u8 {}
fn borrow<'a>(arg: (&'a mut u8,)) -> &'a mut u8 {}
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
                'bools': {'one': 2}, 'caches': {'one': 2}, 'unforbidden': {'one': 1},
                # fields, slot, unit and paired; edit, pick and borrow lend a `&mut` they were
                # given, and with's is a parameter type.
                'mints': {'one': 4},
            })

    def test_a_crate_that_forbids_unsafe_code_counts_zero_unforbidden(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'crates/one/src').mkdir(parents=True)
            (root / 'crates/one/Cargo.toml').write_text('')
            (root / 'crates/one/src/lib.rs').write_text('//! Docs.\n#![forbid(unsafe_code)]\n')
            self.assertEqual(budget.counts(root)['unforbidden'], {'one': 0})

    def test_only_a_rise_over_the_base_fails(self):
        rises = budget.above({'a': 3, 'b': 1, 'c': 2, 'new': 1}, {'a': 2, 'b': 2, 'c': 2})
        self.assertEqual(rises, {'a': (3, 2)}, 'a crate new since the base is not checked')

    def test_an_ffi_crate_may_rise_by_what_its_crate_moved_to_it(self):
        base = {'http': 10, 'http-ffi': 5, 'other': 1, 'orphan-ffi': 1}
        current = {'http': 4, 'http-ffi': 11, 'other': 0, 'orphan-ffi': 2}  # the total holds at 17
        rises = budget.above(current, base)
        self.assertEqual(budget.boundary_moves(rises, current, base), {'orphan-ffi': (2, 1)},
                         'http fell by 6, so http-ffi may rise by 6; orphan has no crate')
        current['http-ffi'] = 12
        self.assertEqual(budget.boundary_moves(budget.above(current, base), current, base),
                         {'http-ffi': (12, 5), 'orphan-ffi': (2, 1)}, 'a rise past the fall fails')

    def test_any_crate_may_rise_while_the_total_falls(self):
        base = {'tls': 3, 'hooks': 5}
        current = {'tls': 4, 'hooks': 3}  # tls gained an unsafe fn; hooks paid for it
        self.assertEqual(budget.boundary_moves(budget.above(current, base), current, base), {})
        current['hooks'] = 4  # the total holds, so tls's rise fails
        self.assertEqual(budget.boundary_moves(budget.above(current, base), current, base), {'tls': (4, 3)})

    def test_a_fall_into_a_new_crate_does_not_also_cover_an_ffi_rise(self):
        # http's 6 moved to locale, which is new and so unchecked; http-ffi's 6 are new unsafe.
        base = {'http': 10, 'http-ffi': 5}
        current = {'http': 4, 'http-ffi': 11, 'locale': 6}
        self.assertEqual(budget.boundary_moves(budget.above(current, base), current, base), {'http-ffi': (11, 5)})

    def test_a_new_crate_counts_from_zero_once_the_total_rises(self):
        base = {'http': 10}
        self.assertEqual(budget.arrivals({'http': 10, 'brotli': 6, 'safe': 0}, base), {'brotli': (6, 0)})
        self.assertEqual(budget.arrivals({'http': 4, 'locale': 6}, base), {}, 'http moved 6 to locale')

    def test_a_new_crate_with_no_unsafe_must_forbid_it(self):
        sources = {'old': '', 'plain': '', 'comment': '// #[unsafe(no_mangle)], unsafe extern "C"\n',
                   'forbids': '#![forbid(unsafe_code)]\n', 'block': 'fn a() { unsafe {} }\n',
                   'export': '#[unsafe(no_mangle)]\nextern "C" fn a() {}\n',
                   'import': 'unsafe extern "C" { safe fn a(); }\n'}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for crate, source in sources.items():
                (root / 'crates' / crate / 'src').mkdir(parents=True)
                (root / 'crates' / crate / 'Cargo.toml').write_text('')
                (root / 'crates' / crate / 'src/lib.rs').write_text(source)
            self.assertEqual(budget.must_forbid(budget.counts(root), {'old': 1}, root),
                             {'plain': (1, 0), 'comment': (1, 0)})


if __name__ == '__main__':
    unittest.main()
