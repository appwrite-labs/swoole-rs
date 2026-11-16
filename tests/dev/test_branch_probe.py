"""The branch-sampling probe counts only real branch-stack entries and hot-function samples."""
import subprocess
import tempfile
import unittest
from unittest.mock import patch
from benchmarks import branch_probe


class Parsing(unittest.TestCase):
    def test_branch_entries_counts_samples_with_a_stack(self):
        script = (' 0x55d5c1a0b1c4/0x55d5c1a0b1a0/P/-/-/2  0x55d5c1a0b1b8/0x55d5c1a0b1c0/M/-/-/1 \n'
                  '\n'
                  ' 0x7f00/0x7f10/P/-/-/0/ANY \n')
        self.assertEqual(branch_probe.branch_entries(script), (2, 3))

    def test_samples_without_a_stack_count_nothing(self):
        self.assertEqual(branch_probe.branch_entries('\n\n'), (0, 0))

    def test_function_samples_sums_the_named_function(self):
        show = ('Function: _ZN4main3hot17h0123456789abcdefE: 51234, 0, 9 sampled lines\n'
                'Function: _ZN4main4main17hfedcba9876543210E: 800, 3, 4 sampled lines\n')
        self.assertEqual(branch_probe.function_samples(show, '3hot'), 51234)
        self.assertEqual(branch_probe.function_samples('', '3hot'), 0)


class Probe(unittest.TestCase):
    def test_a_missing_perf_fails_at_record_and_is_not_verified(self):
        def fake(command, **kwargs):
            if command[0] == 'rustc':
                return subprocess.CompletedProcess(command, 0, '', '')
            return subprocess.CompletedProcess(command, 127, '', 'perf: not found')
        with patch.object(branch_probe, 'run', side_effect=fake), tempfile.TemporaryDirectory() as directory:
            report = branch_probe.probe(directory, convert=True)
        self.assertFalse(report['verified'])
        self.assertEqual(report['failed'], 'record')
        self.assertEqual(report['record']['error'], 'perf: not found')

    def test_a_recording_without_branch_entries_fails_at_decode(self):
        def fake(command, **kwargs):
            out = ' 0x0 \n' if command[:2] == ['perf', 'script'] else ''
            return subprocess.CompletedProcess(command, 0, out, '')
        with patch.object(branch_probe, 'run', side_effect=fake), tempfile.TemporaryDirectory() as directory:
            report = branch_probe.probe(directory)
        self.assertFalse(report['verified'])
        self.assertEqual(report['failed'], 'decode')


if __name__ == '__main__':
    unittest.main()
