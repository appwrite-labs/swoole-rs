"""tools/headers.py fails a hand-written prototype that drifts from cbindgen's."""
import unittest

from tools import headers

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


if __name__ == '__main__':
    unittest.main()
