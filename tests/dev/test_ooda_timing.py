"""Observability preserves failure, isolation and capability outcomes; no native/PHP builds."""
from contextlib import redirect_stdout
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('timed_phpt', Path(__file__).resolve().parents[2] / 'corpus/upstream/run.py')
phpt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(phpt)


class Clock:
    def __init__(self):
        self.value = 0

    def __call__(self):
        self.value += 0.25
        return self.value


class OodaTiming(unittest.TestCase):
    def test_phase_totals_survive_exception_and_accumulate(self):
        for module in [phpt]:
            with self.subTest(module=module.__name__), patch.object(module.time, 'monotonic', Clock()):
                report = {}
                with module.timed_phase(report, 'prepare'):
                    pass
                with self.assertRaisesRegex(RuntimeError, 'original failure'):
                    with module.timed_phase(report, 'prepare'):
                        raise RuntimeError('original failure')
                self.assertEqual(report['timings_seconds']['prepare'], 0.5)

    def test_phpt_retains_both_timings_and_original_statuses(self):
        with tempfile.TemporaryDirectory() as directory:
            sandbox = Path(directory).resolve()
            engine = SimpleNamespace(php='fixture-php', extension=Path('/fixture.so'), php_extensions=(),
                                     command=lambda *args: ['fixture-php', *map(str, args)])
            case = {'path': 'tests/fixture.phpt'}
            for status, code, expected in [('PASSED', 0, 'passed'), ('FAILED', 1, 'failed'), ('SKIPPED', 0, 'skipped')]:
                artifacts = sandbox/status
                def execute(command, **kwargs):
                    self.assertNotIn('-j4', command)
                    (artifacts/'status.txt').write_text(f'{status} {sandbox/case["path"]}\n')
                    (artifacts/'junit.xml').write_text('<testsuite><testcase name="tests/fixture.phpt" time="0.031"/></testsuite>')
                    return SimpleNamespace(stdout='original output', stderr='', returncode=code)
                with self.subTest(status=status), patch.object(phpt, 'run_isolated', side_effect=execute) as run, \
                     patch.object(phpt.time, 'monotonic', Clock()):
                    rows, timing = phpt.run_cases(engine, [case], sandbox, artifacts, 2, 4)
                    run.assert_called_once()
                row = rows[case['path']]
                self.assertEqual(row['status'], expected)
                self.assertEqual(row['timing_basis'], 'isolated_wall_clock')
                self.assertEqual(row['official_runner_test_seconds'], 0.031)
                self.assertEqual(row['isolated_runner_seconds'], 0.25)
                self.assertGreaterEqual(row['seconds'], row['isolated_runner_seconds'])
                self.assertEqual(timing['jobs'], 1)
                self.assertEqual(timing['timings_seconds']['runner'], 0.25)

    def test_phpt_timeout_has_wall_time_without_inventing_junit_time(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            engine = SimpleNamespace(php='fixture-php', extension=Path('/fixture.so'), php_extensions=(), command=lambda *args: ['fixture-php'])
            with patch.object(phpt, 'run_isolated', side_effect=subprocess.TimeoutExpired(['fixture'], 2, output=b'original partial output')), \
                 patch.object(phpt.time, 'monotonic', Clock()):
                rows, timing = phpt.run_cases(engine, [{'path':'tests/fixture.phpt'}], root, root/'artifacts', 2, 1)
            row = rows['tests/fixture.phpt']
            self.assertEqual(row['status'], 'failed')
            self.assertEqual(row['exit_code'], 124)
            self.assertIsNone(row['official_runner_test_seconds'])
            self.assertEqual(row['isolated_runner_seconds'], 0.25)
            self.assertIn('original partial output', Path(row['runner_log']).read_text())
            self.assertEqual(timing['exit_code'], 124)

    def test_phpt_main_checkpoints_preparation_metadata_capability_and_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case = {'path':'tests/fixture.phpt', 'module':'fixture', 'sha256':'fixture', 'requires':[]}
            unsupported = {**case, 'path':'tests/unsupported.phpt', 'requires':['missing']}
            manifest = root/'manifest.json'; manifest.write_text(json.dumps({'commit':phpt.COMMIT, 'tests':[case, unsupported]}))
            output = root/'report.json'
            engine = SimpleNamespace(name='fixture', metadata=Mock(return_value={'capabilities':[]}))
            def prepare(source, sandbox, cases, **kwargs):
                bootstrap = sandbox/'tests/include/bootstrap.php'; bootstrap.parent.mkdir(parents=True)
                bootstrap.write_text('fixture bootstrap'); (sandbox/'run-tests.php').write_text('fixture official runner')
            argv = ['run.py', '--engine', 'stub=/stub', '--source', str(root), '--manifest', str(manifest), '--output', str(output), '--require-all']
            with patch.object(sys, 'argv', argv), patch.object(phpt, 'parse_engines', return_value=[engine]), \
                 patch.object(phpt, 'prepare', side_effect=prepare), \
                 patch.object(phpt.subprocess, 'check_output', return_value='fixture support tree'), \
                 patch.object(phpt, 'run_cases', return_value=({case['path']:{'status':'failed'}}, {'seconds':1})) as execute, \
                 patch.object(phpt.time, 'monotonic', Clock()), redirect_stdout(io.StringIO()):
                self.assertEqual(phpt.main(), 1)
                report = json.loads(output.read_text())
                self.assertEqual([r['status'] for r in report['results']], ['unsupported','failed'])
                self.assertEqual(report['execution_status'], 'complete')
                self.assertEqual(report['engine_timings'][0]['timings_seconds'], {'metadata':0.25, 'capability_selection':0.25, 'cases':0.25})
                self.assertEqual(report['timings_seconds']['prepare'], 0.25)
                execute.assert_called_once()
                engine.metadata.side_effect = RuntimeError('metadata failed')
                with self.assertRaisesRegex(RuntimeError, 'metadata failed'):
                    phpt.main()
                interrupted = json.loads(output.read_text())
                self.assertEqual(interrupted['execution_status'], 'interrupted')
                self.assertEqual(interrupted['engine_timings'][0]['timings_seconds']['metadata'], 0.25)
                with patch.object(phpt, 'prepare', side_effect=ValueError('preparation failed')):
                    with self.assertRaisesRegex(ValueError, 'preparation failed'):
                        phpt.main()
                interrupted = json.loads(output.read_text())
                self.assertEqual(interrupted['execution_status'], 'interrupted')
                self.assertEqual(interrupted['timings_seconds']['prepare'], 0.25)


if __name__ == '__main__':
    unittest.main()
