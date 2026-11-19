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
            (Path(temp) / 'hooks-system-rust.json').write_text(json.dumps({
                'excluded': {'first': 'needs a proxy', 'second': 'needs a proxy'},
                'results': [{'name': name, 'status': 'excluded'} for name in ('first', 'second')]}))
            phases, cases, excluded = summarize(loop)
        self.assertEqual(phases['buffer'], {'passed': 1, 'failed': 1})
        self.assertEqual(phases['shared'], {'passed': 1})
        self.assertEqual(cases['timers'], {'passed': 1, 'unsupported': 1, 'failed': 1})
        self.assertEqual(cases['corpus'], {'passed': 1})
        self.assertEqual(cases['timer'], {'passed': 1, 'failed': 1, 'skipped': 1})
        self.assertEqual(cases['hooks'], {'excluded': 2})
        summary = markdown(phases, cases, excluded)
        self.assertIn('| timers | 1 | 1 | 1 | 0 | 0 |', summary)
        self.assertIn('| needs a proxy | 2 |', summary)


    def test_unreadable_reports_are_annotated_and_lists_skipped(self):
        import os
        from unittest import mock
        from contextlib import redirect_stderr
        import io
        with tempfile.TemporaryDirectory() as temp:
            loop = Path(temp) / 'loop.json'
            loop.write_text(json.dumps({'phases': []}))
            (Path(temp) / 'broken.json').write_text('{"results": [')
            (Path(temp) / 'gate.json').write_text(json.dumps([{'test': 'x', 'status': 'failed'}]))
            (Path(temp) / 'broken.xml').write_text('<testsuites>')
            stderr = io.StringIO()
            with mock.patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), redirect_stderr(stderr):
                _, cases, _ = summarize(loop)
        self.assertEqual(dict(cases), {})
        warnings = [line for line in stderr.getvalue().splitlines() if line.startswith('::warning')]
        self.assertEqual(len(warnings), 2, warnings)
        self.assertTrue(any('broken.json' in line for line in warnings))
        self.assertTrue(any('broken.xml' in line for line in warnings))

if __name__ == '__main__':
    unittest.main()
