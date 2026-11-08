from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.phpt_expectations import select


class VersionedOriginalExpectations(unittest.TestCase):
    def test_both_versions_keep_protected_binary_sections_and_exact_selected_body(self):
        for newline in [b'\n', b'\r\n']:
            protected = newline.join([b'--TEST--', b'case', b'--SKIPIF--', b'<?php die("skip unchanged"); ?>',
                                      b'--FILE--', b'<?php echo "\xff--EXPECTF_85--"; ?>', b''])
            base = b'old %s' + newline + newline
            recent = b'new %s' + newline + b'Stack trace:' + newline
            data = protected + b'--EXPECTF--' + newline + base + b'--EXPECTF_85--' + newline + recent
            for version, body in [(80235, base), (80511, recent)]:
                with self.subTest(newline=newline, version=version):
                    self.assertEqual(select(data, version), protected + b'--EXPECTF--' + newline + body)

    def test_all_three_pinned_kinds_and_exact_80500_threshold(self):
        for kind in [b'EXPECT', b'EXPECTF', b'EXPECTREGEX']:
            data = b'--TEST--\ncase\n--FILE--\nx\n--' + kind + b'--\nbase\n--' + kind + b'_85--\nnew\n'
            self.assertTrue(select(data, 80499).endswith(b'base\n'))
            self.assertTrue(select(data, 80500).endswith(b'new\n'))
            self.assertTrue(select(data, 90000).endswith(b'new\n'))

    def test_version_only_section_does_not_invent_older_expectation(self):
        data = b'--TEST--\ncase\n--FILE--\nx\n--EXPECT_85--\nnew\n'
        with self.assertRaisesRegex(ValueError, 'one active'):
            select(data, 80499)
        self.assertEqual(select(data, 80500), data.replace(b'EXPECT_85', b'EXPECT'))

    def test_duplicate_and_competing_assertions_are_refused(self):
        prefix = b'--TEST--\ncase\n--FILE--\nx\n'
        for tail in [b'--EXPECT--\nx\n--EXPECT--\nx\n', b'--EXPECT--\nx\n--EXPECTF_85--\nx\n']:
            with self.assertRaises(ValueError):
                select(prefix + tail, 80500)

    def test_unconditional_source_is_byte_identical(self):
        data = b'--TEST--\ncase\n--FILE--\nx\n--EXPECT--\ny\n'
        self.assertEqual(select(data, 80500), data)

    def test_invalid_version_and_malformed_source_are_refused(self):
        for version in [None, '80500', 49999]:
            with self.assertRaises(ValueError):
                select(b'--TEST--\ncase\n--EXPECT--\nx\n', version)
        with self.assertRaises(ValueError):
            select(b'prefix\n--TEST--\ncase\n--EXPECT--\nx\n', 80500)


if __name__ == '__main__':
    unittest.main()
