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

from tools import native_core

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
        for module in [native_core, phpt]:
            with self.subTest(module=module.__name__), patch.object(module.time, 'monotonic', Clock()):
                report = {}
                with module.timed_phase(report, 'prepare'):
                    pass
                with self.assertRaisesRegex(RuntimeError, 'original failure'):
                    with module.timed_phase(report, 'prepare'):
                        raise RuntimeError('original failure')
                self.assertEqual(report['timings_seconds']['prepare'], 0.5)

    def test_native_command_checkpoints_success_failure_timeout_and_interrupt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for outcome in [0, 7, subprocess.TimeoutExpired(['fixture'], 2, output=b'original partial stdout', stderr=b'original partial stderr'), KeyboardInterrupt()]:
                report = {'commands': []}
                output = root / 'report.json'
                def checkpoint():
                    output.write_text(json.dumps(report))
                result = SimpleNamespace(stdout='original stdout', stderr='original stderr', returncode=outcome)
                side_effect = outcome if isinstance(outcome, BaseException) else None
                with self.subTest(outcome=outcome), patch.object(native_core.time, 'monotonic', Clock()), \
                     patch.object(native_core, 'run_isolated', return_value=result, side_effect=side_effect) as execute:
                    args = (['fixture'], root, root/'command.log', 2, report, checkpoint, 'configure')
                    if outcome == 0:
                        self.assertEqual(native_core.run_command(*args), 'original stdout')
                    else:
                        expected = type(outcome) if isinstance(outcome, BaseException) else RuntimeError
                        with self.assertRaises(expected):
                            native_core.run_command(*args)
                    execute.assert_called_once_with(['fixture'], cwd=root, timeout=2)
                retained = json.loads(output.read_text())
                self.assertEqual(len(retained['commands']), 1)
                row = retained['commands'][0]
                self.assertEqual(row['seconds'], 0.25)
                self.assertEqual(row['phase'], 'configure')
                self.assertEqual(retained['timings_seconds']['configure'], 0.25)
                if isinstance(outcome, int):
                    self.assertEqual(row['exit'], outcome)
                    self.assertEqual((root/'command.log').read_text(), 'original stdoutoriginal stderr')
                else:
                    self.assertIn('error', row)
                    self.assertNotIn('exit', row)
                    self.assertEqual(row['error_type'], type(outcome).__name__)
                    if isinstance(outcome, subprocess.TimeoutExpired):
                        self.assertTrue(row['timed_out'])
                        self.assertEqual((root/'command.log').read_text(), 'original partial stdoutoriginal partial stderr')
                        self.assertEqual(row['log_sha256'], native_core.sha(root/'command.log'))

    def test_native_cache_reasons_and_failed_preparation_are_durable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'migration').mkdir()
            (root/'migration/modules.json').write_text('{"modules": []}')
            (root/'tools').mkdir(); (root/'tools/build_hybrid.py').write_text('fixture recipe')
            config = root/'config.h'; config.write_text('fixture config')
            output = root/'report.json'
            source = root/'source'; source.mkdir()
            argv = ['native_core.py', '--source', str(source), '--config', str(config),
                    '--cache', str(root/'cache'), '--output', str(output)]
            with patch.object(sys, 'argv', argv), patch.object(native_core, 'ROOT', root), \
                 patch.object(native_core, 'require_build_space'), patch.object(native_core, 'validate'), \
                 patch.object(native_core.platform, 'platform', return_value='fixture OS'), \
                 patch.object(native_core.subprocess, 'check_output', return_value='fixture version\n'), \
                 patch.object(native_core.subprocess, 'run', return_value=SimpleNamespace(returncode=0, stdout='source', stderr='')), \
                 patch.object(native_core, 'stage', side_effect=ValueError('fixture preparation failed')) as stage, \
                 patch.object(native_core, 'run_isolated') as execute, \
                 patch.object(native_core.time, 'monotonic', Clock()), redirect_stdout(io.StringIO()):
                self.assertEqual(native_core.main(), 1)
                report = json.loads(output.read_text())
                self.assertIn('cache_hit', report, report)
                self.assertFalse(report['cache_hit'])
                self.assertEqual(report['cache_reason'], 'no build receipt for this exact input identity')
                self.assertEqual(report['status'], 'failed')
                self.assertGreater(report['timings_seconds']['prepare'], 0)
                execute.assert_not_called()
                work = Path(report['cache'])
                binary = work/'build/bin/core-tests'; binary.parent.mkdir(parents=True); binary.write_bytes(b'fixture')
                library = work/'src/lib'/('libswoole.dylib' if native_core.platform.system() == 'Darwin' else 'libswoole.so')
                library.parent.mkdir(parents=True); library.write_bytes(b'fixture library')
                receipt = work/'build-receipt.json'
                receipt.write_text(json.dumps({'binary_sha256': native_core.sha(binary),
                                              'native_library': native_core.library_identity(library), 'assertions': {}}))
                stage.reset_mock()
                with patch.object(native_core, 'verify_loader', return_value={}), \
                     patch.object(native_core, 'run_isolated', return_value=SimpleNamespace(stdout='fixture.\n  original\n', stderr='', returncode=0)) as listing:
                    self.assertEqual(native_core.main(), 0)
                    listing.assert_called_once()
                hit = json.loads(output.read_text())
                self.assertTrue(hit['cache_hit'])
                self.assertEqual(hit['cache_reason'], 'verified executable and native-library identities')
                self.assertEqual(hit['results'], [])
                self.assertIn('inventory', hit['timings_seconds'])
                self.assertGreater(hit['timings_seconds']['cache_validation'], 0)
                stage.assert_not_called()
                argv.extend(['--case', 'fixture.original'])
                for outcome in ['passed', 'failed', 'skipped', 'timeout']:
                    def original(command, **kwargs):
                        if '--gtest_list_tests' in command:
                            return SimpleNamespace(stdout='fixture.\n  original\n', stderr='', returncode=0)
                        if outcome == 'timeout':
                            raise subprocess.TimeoutExpired(command, 2, output='original partial output')
                        xml = Path(next(arg.split('xml:', 1)[1] for arg in command if '--gtest_output=xml:' in arg))
                        detail = {'passed':'', 'failed':'<failure/>', 'skipped':'<skipped/>'}[outcome]
                        xml.write_text('<testsuite><testcase classname="fixture" name="original" status="run" result="completed">'+detail+'</testcase></testsuite>')
                        return SimpleNamespace(stdout='original output', stderr='', returncode=int(outcome == 'failed'))
                    with patch.object(native_core, 'verify_loader', return_value={}), \
                         patch.object(native_core, 'run_isolated', side_effect=original) as execute_case:
                        self.assertEqual(native_core.main(), int(outcome != 'passed'))
                        self.assertEqual(execute_case.call_count, 2)
                    actual = json.loads(output.read_text())
                    self.assertEqual(actual['results'][0]['status'], 'failed' if outcome == 'timeout' else outcome)
                    self.assertGreater(actual['results'][0]['seconds'], 0)
                    self.assertEqual(actual['timings_seconds']['cases'], actual['results'][0]['seconds'])
                    self.assertEqual(actual['unexecuted'], [])
                del argv[-2:]
                library.write_bytes(b'changed')
                self.assertEqual(native_core.main(), 1)
                bad = json.loads(output.read_text())
                self.assertFalse(bad['cache_hit'])
                self.assertIn('mismatch', bad['cache_reason'])
                self.assertEqual(bad['status'], 'failed')
                stage.assert_not_called()

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
