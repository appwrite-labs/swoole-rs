"""Read-only diagnostics preserve statuses, bounded output and portable log paths."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import dev_report


class Reports(unittest.TestCase):
    def fixture(self, directory, status='failed'):
        directory = Path(directory)
        (directory / 'model.json').write_text(json.dumps({'fixture_compile': {'cache_hit': True, 'cache_reason': 'matches'},
                                                         'engines': {'published': {'compile': {'cache_hit': False, 'cache_reason': 'header changed'}}}}))
        (directory / 'phpt.json').write_text(json.dumps({'results': [
            {'engine': 'hybrid', 'test': 'original.phpt', 'status': 'failed', 'detail': 'expected output mismatch'},
            {'engine': 'hybrid', 'test': 'memory.phpt', 'status': 'skipped', 'detail': 'no top'},
            {'engine': 'prototype', 'test': 'missing.phpt', 'status': 'unsupported'}]}))
        (directory / 'phpt.log').write_text('earlier output\n' + 'x' * 20000 + '\nlast failure\n')
        data = {'lane': 'integrate', 'status': status, 'hypothesis': 'same behavior',
                'expected_observation': 'original failures visible', 'phases': [
                    {'phase': 'model', 'seconds': 1.2, 'returncode': 0, 'log': '/work/reports/model.log',
                     'command': ['python3', 'model.py', '--output', '/work/reports/model.json']},
                    {'phase': 'phpt', 'seconds': 12.3, 'returncode': 0 if status == 'passed' else 1, 'log': '/work/reports/phpt.log',
                     'command': ['python3', 'run.py', '--output', '/work/reports/phpt.json']}]}
        if status != 'running':
            data['seconds'] = 13.6
        path = directory / 'loop.json'
        path.write_text(json.dumps(data))
        return path

    def test_ranked_phases_nested_caches_and_original_statuses(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.fixture(directory)
            before = {p.name: p.read_bytes() for p in path.parent.iterdir()}
            report = dev_report.inspect(path)
            self.assertEqual(report['status'], 'failed')
            self.assertEqual([r['phase'] for r in report['slowest']], ['phpt', 'model'])
            self.assertEqual([r['hit'] for r in report['slowest'][1]['caches']], [True, False])
            self.assertEqual([r['status'] for r in report['failures'][0]['problems']], ['failed', 'skipped', 'unsupported'])
            self.assertEqual(report['failures'][0]['excerpt'][-12:], 'last failure')
            self.assertLessEqual(len(report['failures'][0]['excerpt']), 4000)
            self.assertEqual(before, {p.name: p.read_bytes() for p in path.parent.iterdir()})

    def test_running_is_not_reported_as_passed(self):
        with tempfile.TemporaryDirectory() as directory:
            report = dev_report.inspect(self.fixture(directory, 'running'), limit=1)
            self.assertEqual(report['status'], 'running')
            self.assertIsNone(report['seconds'])
            self.assertEqual(len(report['slowest']), 1)

    def test_missing_logs_and_malformed_artifacts_remain_visible(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.fixture(directory)
            (path.parent / 'phpt.log').unlink()
            for invalid in ['{', '[]']:
                (path.parent / 'phpt.json').write_text(invalid)
                report = dev_report.inspect(path)
                self.assertTrue(report['slowest'][0]['warnings'])
                self.assertIn('Log unavailable', report['failures'][0]['excerpt'])

    def test_latest_and_cli_exit_status(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            old = root / 'old'
            old.mkdir()
            path = self.fixture(old)
            self.assertEqual(dev_report.latest(root), path)
            command = [sys.executable, str(dev_report.ROOT / 'tools/dev_report.py'), str(path), '--json']
            for status, code in [('failed', 1), ('running', 2), ('passed', 0)]:
                self.fixture(old, status)
                result = subprocess.run(command, capture_output=True, text=True)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(json.loads(result.stdout)['status'], status)
            newer = root / 'newer'
            newer.mkdir()
            candidate = self.fixture(newer, 'passed')
            data = json.loads(candidate.read_text())
            data['lane'] = 'candidate'
            candidate.write_text(json.dumps(data))
            os.utime(path, ns=(1, 1))
            os.utime(candidate, ns=(2, 2))
            self.assertEqual(dev_report.latest(root), candidate)
            self.assertEqual(dev_report.latest(root, 'integrate'), path)

    def test_contradictory_success_cannot_hide_a_failed_phase(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.fixture(directory)
            data = json.loads(path.read_text())
            data['status'] = 'passed'
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, 'failed phase'):
                dev_report.inspect(path)


if __name__ == '__main__':
    unittest.main(verbosity=2)
