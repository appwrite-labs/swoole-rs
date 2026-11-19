"""PHPT clearance evidence, run denominators and Cargo-only filing."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import Mock

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.phpt_trends import file_issues, read_evidence, trends


class Trends(unittest.TestCase):
    def test_historical_artifact_classification(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            self.assertIsNone(read_evidence(directory))
            rows = [dict(test=test, engine=engine, status=status, module='m')
                    for test in ('cargo', 'shared', 'still-broken', 'inconclusive')
                    for engine, status in [('cargo', 'failed'), ('upstream', 'passed')]]
            (directory / 'phpt.json').write_text(json.dumps(rows))
            for row in rows:
                row['status'] = 'failed' if (row['test'], row['engine']) in (
                    ('shared', 'upstream'), ('still-broken', 'cargo'), ('inconclusive', 'cargo')) else 'passed'
                if row['test'] == 'inconclusive' and row['engine'] == 'cargo':
                    row.update(upstream_status='PASSED', detail='corrupted double-linked list Aborted')
            (directory / 'recheck').mkdir()
            (directory / 'recheck' / 'results.json').write_text(json.dumps({'results': rows}))
            self.assertEqual(read_evidence(directory), dict(cargo_only=['cargo'], also_upstream=['shared'],
                                                           upstream_failed=['shared']))

    def test_threshold_missing_runs_and_cross_run_upstream_failure(self):
        runs = [{'databaseId': i, 'url': f'run-{i}'} for i in range(3)]
        def gh(*args):
            if args[1] == 'list':
                return json.dumps(runs)
            if args[2] == '2':
                return  # partial shard artifact
            data = dict(cargo_only=['a', 'b'] if args[2] == '0' else ['a'],
                        also_upstream=[], upstream_failed=['a'] if args[2] == '1' else [])
            (Path(args[-1]) / 'clearances.json').write_text(json.dumps(data))
        report = trends(gh, 3, 0.5)
        self.assertEqual(report['missing'], ['run-2'])
        self.assertEqual(report['measured'], 2)
        self.assertEqual(report['tests'], [dict(test='a', cleared=2, upstream_also_failed=True,
                                              cargo_only=['run-0', 'run-1'], also_upstream=[])])
        unused = Mock()
        file_issues(unused, report)
        unused.assert_not_called()

    def test_file_only_new_cargo_bugs(self):
        def gh(*args):
            if args[1] == 'list':
                return json.dumps([{'title': 'tracked', 'body': 'tests/b.phpt'}])
            self.assertIn('run-a', Path(args[args.index('--body-file') + 1]).read_text())
            self.assertIn('kind:bug,p2,status:ready', args)
            self.assertIn('loks0n', args)
            return 'new-issue'
        rows = [dict(test=f'tests/{name}.phpt', cleared=2, cargo_only=['run-a', 'run-b'],
                     upstream_also_failed=name == 'c') for name in 'abc']
        file_issues(gh, dict(requested=5, measured=5, tests=rows))
        self.assertEqual([row.get('issue') for row in rows], ['new-issue', None, None])
