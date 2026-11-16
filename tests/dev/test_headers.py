"""tools/headers.py fails a hand-written prototype that drifts from cbindgen's, and
tools/externs.py a hand-written extern declaration that drifts from the C header."""
import shutil
import tempfile
import unittest
from pathlib import Path

from tools import externs, headers

GENERATED = """
typedef struct WaitOps WaitOps;
bool swr_v1_wait(const WaitOps *ops, void *co, uint32_t event, Barrier barrier, uintptr_t length);
intptr_t swr_v1_read(uint8_t *out, uintptr_t length);
"""

HEADER = """
#ifdef __cplusplus
extern "C" {
#endif
typedef struct { int x; } swr_v1_wait_ops;
bool swr_v1_wait(const swr_v1_wait_ops *ops, void *co, uint32_t event, swr_v1_barrier barrier, size_t length);
ssize_t swr_v1_read(char *out, size_t length);
static inline int helper(void) { return swr_v1_read(nullptr, 0) > 0; }
"""


def run(header, generated=GENERATED, sources=('',)):
    exported = {name: ('demo', proto) for name, proto in headers.prototypes(generated).items()}
    return headers.check({'native/demo.h': header}, exported, list(sources))


class Headers(unittest.TestCase):
    def test_matching_prototypes_pass(self):
        self.assertEqual(run(HEADER), [])

    def test_a_changed_parameter_type_fails(self):
        problems = run(HEADER.replace('uint32_t event', 'uint64_t event'))
        self.assertEqual(problems, ['native/demo.h: swr_v1_wait parameter 3 (event) is u64; crates/demo has u32'])

    def test_lost_constness_fails(self):
        self.assertEqual(len(run(HEADER.replace('const swr_v1_wait_ops', 'swr_v1_wait_ops'))), 1)

    def test_a_renamed_parameter_fails(self):
        self.assertEqual(run(HEADER.replace('size_t length);\nssize_t', 'size_t n);\nssize_t')),
                         ['native/demo.h: swr_v1_wait parameter 5 is named n; crates/demo names it length'])

    def test_a_removed_export_fails(self):
        generated = GENERATED.replace('intptr_t swr_v1_read(uint8_t *out, uintptr_t length);', '')
        self.assertEqual(run(HEADER, generated), ['native/demo.h: swr_v1_read is not exported by any crate'])

    def test_a_later_abi_version_is_checked_too(self):
        generated = GENERATED + 'void *swr_v2_open(const uint8_t *data, uintptr_t length);\n'
        self.assertEqual(run(HEADER + 'void *swr_v2_open(const uint8_t *data);\n', generated),
                         ['native/demo.h: swr_v2_open takes 1 parameters; crates/demo takes 2'])

    def test_a_macro_generated_export_is_only_named(self):
        generated = GENERATED.replace('intptr_t swr_v1_read(uint8_t *out, uintptr_t length);', '')
        self.assertEqual(run(HEADER, generated, ['export!(swr_v1_read, u8);']), [])


C_HEADER = """
#include <stddef.h>
typedef struct table table;
typedef struct { const char *ptr; size_t len; } bytes;
#ifdef __cplusplus
extern "C" {
#endif
long demo_add(table *t, const char *key, unsigned flags);
void demo_log(const char *format, ...);
bytes demo_slice(bytes in, double scale);
#ifdef __cplusplus
}
#endif
"""

RUST = """
use std::ffi::{c_char, c_uint};
#[repr(transparent)]
struct Table<'a>(&'a Opaque, PhantomData<&'a ()>);
#[repr(transparent)]
struct Flags<T = c_uint>(T);
#[repr(C)]
struct Bytes { ptr: *const u8, len: usize }
unsafe extern "C" {
    /// Documented.
    safe fn demo_add(table: Table<'_>, key: *const c_char, flags: Flags) -> i64;
    fn demo_log(format: *const c_char, ...);
    safe fn demo_slice(input: Bytes, scale: f64) -> Bytes;
    #[cfg(target_os = "macos")]
    safe fn demo_add(table: Table<'_>) -> i64;
    safe fn not_in_c(x: u8);
}
"""


@unittest.skipUnless(shutil.which('g++'), 'g++ required')
class Externs(unittest.TestCase):
    def check(self, rust):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, 'demo.h').write_text(C_HEADER)
            return externs.check({externs.ROOT / 'crates/demo/src/lib.rs': rust}, '#include "demo.h"\n', [f'-I{tmp}'])

    def test_matching_declarations_pass(self):
        self.assertEqual(self.check(RUST), ([], ['crates/demo/src/lib.rs:16: not_in_c']))

    def test_a_wrong_width_fails(self):
        problems, _ = self.check(RUST.replace('flags: Flags)', 'flags: Flags<u64>)'))
        self.assertEqual(problems, ['crates/demo/src/lib.rs:11: demo_add: the C prototype is not 8pp8'])

    def test_a_wrong_float_width_fails(self):
        problems, _ = self.check(RUST.replace('scale: f64', 'scale: f32'))
        self.assertEqual(problems, ['crates/demo/src/lib.rs:13: demo_slice: the C prototype is not aaf'])

    def test_an_integer_for_a_pointer_a_missing_parameter_and_a_lost_variadic_fail(self):
        rust = RUST.replace('key: *const c_char', 'key: usize').replace(', ...)', ')')
        problems, _ = self.check(rust.replace('input: Bytes, ', ''))
        self.assertEqual(problems, ['crates/demo/src/lib.rs:11: demo_add: the C prototype is not 8p84',
                                    'crates/demo/src/lib.rs:12: demo_log: the C prototype is not vp',
                                    'crates/demo/src/lib.rs:13: demo_slice: the C prototype is not ad'])

    def test_an_unknown_type_fails(self):
        problems, _ = self.check(RUST.replace('flags: Flags)', 'flags: Mystery)'))
        self.assertEqual(problems, ['crates/demo/src/lib.rs:11: demo_add: no shape for Mystery'])


if __name__ == '__main__':
    unittest.main()
