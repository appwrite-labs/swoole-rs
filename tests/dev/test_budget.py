"""tools/budget.py counts unsafe items and slow data layouts in non-test crate code; no workspace total may rise over the base commit."""
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
fn flags(open: Vec<bool>, fixed: [bool; 4], full: std::vec::Vec<bool>) {}
pub fn fields(self) -> &'static mut Record {}
pub(crate) fn slot(&self, index: usize) -> Option<&mut u8> {}
fn edit(&mut self) -> &mut u8 {}
fn pick<'a>(slots: &'a mut [u8]) -> &'a mut u8 {}
fn with(field: fn(&mut Record) -> &mut u8) {}
fn unit(_: ()) -> &'static mut u8 {}
fn paired(pair: (u8, u8)) -> &'static mut u8 {}
fn borrow<'a>(arg: (&'a mut u8,)) -> &'a mut u8 {}
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "C's")]
fn narrow(x: i64) -> u8 { x as u8 }
#[cfg(not(test))]
static NATIVE: Ops = OPS;
#[cfg(all(feature = "upstream-symbols", not(test)))]
mod linked;
#![cfg_attr(
    not(test),
    deny(clippy::undocumented_unsafe_blocks)
)]
use std::cell::RefCell;
thread_local! { static STATE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) }; }
struct Owner { queue: std::cell::RefCell<Vec<u8>> }
fn discarded() {
    let _status = close();
    // The destructor checks the final status.
    let _sent = send();
    let _guard = lock();
    let _guard_mask = mask();
}
fn owner() -> Owner { Owner { queue: RefCell::default() } }
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
            test_source = 'unsafe {}\nfn status() -> i32 { 0 }\n#[repr(C)]\nstruct Test;\n'
            (src / 'imp/tests.rs').write_text(test_source)
            (src / 'loaded_tests.rs').write_text(test_source)
            (root / 'crates/one/tests').mkdir()
            (root / 'crates/one/tests/it.rs').write_text(test_source)
            (src / 'shim.c').write_text('#include "shim.h"\n\nint one(void) { return 1; }\n')
            (src / 'shim.h').write_text('int one(void);\n')
            (root / 'crates/one/tests/stub.c').write_text('int stub(void);\n')
            # a, its block, c, the impl, the cfg(test) helper's block, imp's block.
            self.assertEqual(budget.counts(root), {
                'unsafe': {'one': 6}, 'maps': {'one': 1}, 'lists': {'one': 1},
                'bools': {'one': 3}, 'caches': {'one': 2}, 'unforbidden': {'one': 1},
                # fields, slot, unit and paired; edit, pick and borrow lend a `&mut` they were
                # given, and with's is a parameter type.
                'mints': {'one': 4},
                # NATIVE and linked; the lint attribute isn't a cfg.
                'fakes': {'one': 2},
                # narrow's two; spanned's attribute belongs to a test module.
                'casts': {'one': 2},
                # STATE and queue; the import and the constructors name no type.
                'cells': {'one': 2},
                'statuses': {'one': 0}, 'reprs': {'one': 0}, 'derefs': {'one': 0},
                'externs': {'one': 0}, 'shortreads': {'one': 0}, 'rawtemp': {'one': 0}, 'unreasoned': {'one': 1},
                # shim.c's three and shim.h's one; tests/ isn't src/.
                'clines': {'one': 4}, 'php_args': {},
            })

    def test_raw_temp_file_calls_belong_to_the_guard(self):
        with tempfile.TemporaryDirectory() as root:
            for name, expected in [('crates/corebase/src/ossupport/temp_file.rs', 0),
                                   ('crates/http/src/request.rs', 2)]:
                path = Path(root) / name
                path.parent.mkdir(parents=True)
                path.write_text('fn open(p: *mut i8) { unsafe { libc::mkstemp(p); mkostemp(p, 0); } }')
                self.assertEqual(budget.count_file(path, set())['rawtemp'], expected)

    def test_short_reads_only_count_in_wire_readers(self):
        with tempfile.TemporaryDirectory() as directory:
            for crate, expected in [('http2', 2), ('other', 0)]:
                path = Path(directory) / 'crates' / crate / 'src/lib.rs'
                path.parent.mkdir(parents=True)
                path.write_text("""
fn decode(bytes: &[u8]) { bytes.get(4..).unwrap_or_default(); }
fn read_u32(input: &[u8]) { for (a,b) in raw.iter_mut().zip(input) { *a = *b; } }
fn encode(bytes: &[u8]) { bytes.get(4..).unwrap_or_default(); }
""")
                self.assertEqual(budget.count_file(path, set())['shortreads'], expected)

    def test_safe_fns_whose_unsafe_blocks_use_a_raw_pointer_parameter(self):
        source = '''\
fn read(p: *mut u8) -> u8 { unsafe { *p } }
pub(crate) extern "C" fn callback(data: *mut c_void) { unsafe { run(data.cast()) } }
fn split(
    cb: fn(*mut u8) -> i32,
    mut at: Option<*const Record>,
) {
    if let Some(at) = at { unsafe { (*at).go() } }
}
unsafe fn contract(p: *mut u8) -> u8 { unsafe { *p } }
unsafe extern "C" fn native(p: *mut u8) -> u8 { unsafe { *p } }
fn compared(slot: &AtomicPtr<u8>, ours: *mut u8) -> bool { let t = slot.swap(ours); unsafe { drop_raw(t) } }
fn stored(&mut self, p: *mut u8) { self.p = p; }
fn callback_type(cb: extern "C" fn(*mut u8), n: usize) { unsafe { cb(n) } }
fn declared(p: *mut u8);
unsafe extern "C" {
    safe fn raw_arg(p: *mut u8, n: usize) -> i32;
    pub(crate) safe fn maybe(
        at: Option<*const Record>,
    );
    safe fn by_ref(out: &mut *mut c_char) -> i32;
    safe fn with_callback(cb: extern "C" fn(*mut u8));
    fn unsafe_one(p: *mut u8);
}
#[cfg(test)]
mod tests {
    fn helper(p: *mut u8) -> u8 { unsafe { *p } }
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lib.rs'
            path.write_text(source)
            # read, callback and split; the unsafe fns state a contract, compared's and
            # stored's blocks don't use the pointer, and callback_type's is a parameter type.
            counted = budget.count_file(path, set())
            self.assertEqual(counted['derefs'], 3)
            # raw_arg and maybe: by_ref takes a reference, with_callback's pointer is in a
            # parameter type, and unsafe_one's callers write `unsafe`.
            self.assertEqual(counted['externs'], 2)

    def test_status_bodies_and_c_layouts_exclude_test_modules(self):
        source = '''\
fn status() -> i32 { 0 }
pub unsafe extern "C" fn wrapper() -> c_int { 0 }
fn wide() -> i64 { 0 }
fn multiline(
    callback: fn() -> i32,
) -> isize { 0 }
fn split() ->
    i32
{ 0 }
fn generic<T>() -> i64
where T: Copy
{ 0 }
fn typed() -> Result<i32, Error> { todo!() }
fn unsigned() -> usize { 0 }
unsafe extern "C" { fn foreign() -> c_int; }
type Callback = extern "C" fn() -> c_int;
#[repr(C)]
struct Record;
#[repr(C, packed)]
struct Packed;
#[repr(
    align(8), C
)]
struct Aligned;
#[repr(transparent)]
struct Handle;
// fn comment() -> i32 { 0 } #[repr(C)]
#[cfg(test)]
mod tests {
    fn helper() -> i32 { 0 }
    #[repr(C)]
    struct Test;
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lib.rs'
            path.write_text(source)
            counted = budget.count_file(path, set())
            self.assertEqual(counted['statuses'], 6)
            self.assertEqual(counted['reprs'], 3)

    def test_a_macro_body_counts_by_its_tokens(self):
        source = '''\
macro_rules! exports {
    ($($name:ident = $call:expr;)*) => {$(
        #[repr(C)]
        struct Record;
        pub unsafe extern "C" fn $name(flags: Vec<bool>) -> c_int { unsafe { $call } }
        unsafe fn inner() -> Option<i32> { None }
        unsafe impl Send for Record {}
        static NAME_CACHE: RefCell<[bool; 2]> = RefCell::new([false; 2]);
        // unsafe { not code }
    )*};
}
per_thread! { static SEEN: HashSet<u8> = HashSet::new(); }
fn checked() { assert!(unsafe { ready() }, "unsafe { text }"); }
type Routine = unsafe extern "C" fn();
unsafe extern "C" { fn foreign(); }
#[cfg(test)]
mod tests {
    fn t() { assert!(unsafe { skipped() }); }
}
'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lib.rs'
            path.write_text(source)
            counted = budget.count_file(path, set())
            # $name, its block, inner, the impl and checked's block; Routine is a type and the
            # extern block declares.
            self.assertEqual(counted['unsafe'], 5)
            self.assertEqual({kind: counted[kind] for kind in ('reprs', 'statuses', 'caches', 'cells', 'bools', 'maps')},
                             {'reprs': 1, 'statuses': 1, 'caches': 1, 'cells': 1, 'bools': 2, 'maps': 2})

    def test_a_crate_that_forbids_unsafe_code_counts_zero_unforbidden(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'crates/one/src').mkdir(parents=True)
            (root / 'crates/one/Cargo.toml').write_text('')
            (root / 'crates/one/src/lib.rs').write_text('//! Docs.\n#![forbid(unsafe_code)]\n')
            self.assertEqual(budget.counts(root)['unforbidden'], {'one': 0})

    def test_a_c_rise_names_reasons_what_stays_c_lists(self):
        with tempfile.TemporaryDirectory() as directory:
            zend = Path(directory) / 'zend.md'
            zend.write_text('## What stays C\n\n- **bailout**: setjmp.\n- **globals**: EG.\n\n## Next\n- **other**: no.\n')
            self.assertEqual(budget.stays_c('Closes #1.\nStays C: bailout, globals\n', zend), ['bailout', 'globals'])
            self.assertEqual(budget.stays_c('Stays C: bailout\nStays C: other\n', zend), [], 'other is not a reason')
            self.assertEqual(budget.stays_c('It stays C: bailout', zend), [], 'only a line of its own')
            self.assertEqual(budget.stays_c('', zend), [])
        self.assertTrue(budget.stays_c('Stays C: variadic'), "docs/zend.md's own list")

    def test_only_a_rise_of_the_workspace_total_fails(self):
        def kinds(unsafe, clines=None, unforbidden=None):
            return {'unsafe': unsafe, 'clines': clines or {}, 'unforbidden': unforbidden or {}}
        base = kinds({'hooks': 7, 'corebase': 153, 'gone': 4})
        # A module moves between surviving crates, a fold deletes one, a new crate takes items:
        # none adds to the workspace (#4733).
        for moved in ({'hooks': 0, 'corebase': 160, 'gone': 4}, {'hooks': 7, 'corebase': 157},
                      {'hooks': 3, 'corebase': 153, 'gone': 4, 'new': 4}, {'hooks': 9, 'corebase': 150, 'gone': 4}):
            self.assertEqual(budget.failures(kinds(moved), base), {}, moved)
        self.assertEqual(budget.failures(kinds({'hooks': 1, 'corebase': 160, 'gone': 4, 'new': 1}), base),
                         {'unsafe': ['unsafe: 166 in the workspace, up from 164',
                                     '  corebase: 160, up from 153', '  new: 1, up from 0']})
        c_base = kinds({}, {'zend': 10})
        self.assertIn('clines', budget.failures(kinds({}, {'zend': 11}), c_base))
        self.assertEqual(budget.failures(kinds({}, {'zend': 11}), c_base, 'Stays C: variadic'), {})
        self.assertEqual(budget.failures(kinds({}, unforbidden={'a': 1, 'b': 0}), kinds({}, unforbidden={'a': 0, 'b': 1})),
                         {'unforbidden': ['a: 1 unforbidden, up from 0']}, 'a crate that stops forbidding fails alone')

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
