"""The merge-tier summary counts phases per module and Cargo-engine cases per group, never folding unsupported into passed."""
import json
from pathlib import Path
import tempfile
import unittest

from tools.merge_summary import markdown, summarize


class MergeSummary(unittest.TestCase):
    def test_counts(self):
        with tempfile.TemporaryDirectory() as temp:
            loop = Path(temp) / 'loop.json'
            loop.write_text(json.dumps({'phases': [
                {'phase': 'buffer-model', 'returncode': 0}, {'phase': 'string-copy', 'returncode': 1},
                {'phase': 'format', 'returncode': 0}]}))
            (Path(temp) / 'phpt-originals.json').write_text(json.dumps({'results': [
                {'engine': 'cargo', 'module': 'timers', 'status': 'passed'},
                {'engine': 'cargo', 'module': 'timers', 'status': 'unsupported'},
                {'engine': 'cargo', 'module': 'timers', 'status': 'xfail'},
                {'engine': 'upstream', 'module': 'timers', 'status': 'failed'}]}))
            (Path(temp) / 'applications.json').write_text(json.dumps({'results': [
                {'engine': 'cargo', 'project': '01-hello', 'status': 'passed'}]}))
            (Path(temp) / 'timer-originals.xml').write_text(
                '<testsuites><testsuite><testcase name="test_original[after-cargo]"/>'
                '<testcase name="test_original[stats-cargo]"><failure/></testcase>'
                '<testcase name="test_original[stats-upstream]"><failure/></testcase>'
                '<testcase name="test_original[bug-cargo]"><skipped/></testcase></testsuite></testsuites>')
            phases, cases = summarize(loop)
        self.assertEqual(phases['buffer'], {'passed': 1, 'failed': 1})
        self.assertEqual(phases['shared'], {'passed': 1})
        self.assertEqual(cases['timers'], {'passed': 1, 'unsupported': 1, 'failed': 1})
        self.assertEqual(cases['corpus'], {'passed': 1})
        self.assertEqual(cases['timer'], {'passed': 1, 'failed': 1, 'skipped': 1})
        self.assertIn('| timers | 1 | 1 | 1 | 0 |', markdown(phases, cases))


if __name__ == '__main__':
    unittest.main()
