"""Regression checks for cache invalidation, isolation, selection and PHPT failures."""
import importlib.util
import json
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import build_ext, build_upstream, dev, harness
from tools.harness import Engine

spec = importlib.util.spec_from_file_location('phpt_runner', ROOT / 'corpus/upstream/run.py')
phpt = importlib.util.module_from_spec(spec)
spec.loader.exec_module(phpt)


class Exclusive(unittest.TestCase):
    def test_exclusive_writer(self):
        with tempfile.TemporaryDirectory() as directory:
            lock = Path(directory) / 'writer.lock'
            with harness.exclusive(lock):
                with self.assertRaisesRegex(RuntimeError, 'Another writer'):
                    with harness.exclusive(lock):
                        self.fail('Concurrent writer acquired lock')


class Overlays(unittest.TestCase):
    def test_only_selected_contracts_are_staged_and_collisions_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['include', 'websocket', 'buffer']:
                (root / 'native' / name).mkdir(parents=True)
            for name in ['include/swoole_rs.h', 'include/swoole_rs_websocket.h',
                         'websocket/codec.h', 'buffer/string.h', 'include/swoole_rs_corebase_constants.h']:
                (root / 'native' / name).touch()
            with patch.object(build_ext, 'ROOT', root):
                selected = build_ext.headers_for(['websocket'])
                self.assertEqual({p.name for p in selected}, {'swoole_rs.h', 'swoole_rs_server_state.h', 'swoole_rs_server_view.h', 'swoole_rs_server_records.h', 'swoole_rs_port_state.h', 'swoole_rs_server_shared.h', 'swoole_rs_server_strings.h', 'swoole_rs_websocket.h', 'codec.h', 'swoole_rs_corebase_constants.h'})
                (root / 'native/websocket/swoole_rs.h').touch()
                with self.assertRaises(ValueError):
                    build_ext.headers_for(['websocket'])

class Phpt(unittest.TestCase):
    def test_manifest_lists_every_pinned_timer_original(self):
        tests = json.loads((ROOT / 'corpus/upstream/manifest.json').read_text())['tests']
        timer = {Path(case['path']).stem: case for case in tests if case['path'].startswith('tests/swoole_timer/')}
        self.assertEqual(len(timer), 35)
        self.assertEqual({name for name, case in timer.items() if case.get('network')},
                         {'bug_4794_3', 'bug_4794_5', 'bug_4794_6', 'bug_4794_7'})
        self.assertTrue(all(case['module'] == 'timers' for case in timer.values()))
        self.assertEqual(len({case['path'] for case in tests}), len(tests))

    def test_fixed_port_guard_refuses_live_listener(self):
        import socket
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            listener.listen()
            port = listener.getsockname()[1]
            with self.assertRaises(OSError):
                phpt.require_free_port(port)
        phpt.require_free_port(port)

    def test_pinned_support_assets_ignore_working_copy_and_reject_bad_inputs(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            tree = Path(directory)
            source = tree / 'source'
            source.mkdir()
            remote = "// Run default remote object server\nswoole_library_set_option('default_remote_object_server_worker_num', 2);\nswoole_init_default_remote_object_server();\n"
            files = {'.gitattributes': b'examples/binary.bin export-ignore\n', 'LICENSE': b'fixture', 'tools/bootstrap.php': b'<?php',
                     'tests/include/bootstrap.php': remote.encode(),
                     'tests/original.phpt': b'--TEST--\noriginal\n--FILE--\n<?php echo "ok";\n--EXPECT--\nok',
                     'examples/binary.bin': bytes(range(256)), 'examples/empty.txt': b''}
            for name, content in files.items():
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            subprocess.run(['git', 'init', '-q', str(source)], check=True)
            subprocess.run(['git', '-C', str(source), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(source), '-c', 'user.name=Fixture', '-c',
                            'user.email=fixture@local.invalid', 'commit', '-qm', 'fixture'], check=True)
            pin = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
            (source / 'run-tests.php').write_text('<?php // official runner copy fixture')
            (source / 'examples/binary.bin').write_bytes(b'changed working copy')
            cases = [{'path': 'tests/original.phpt', 'sha256': hashlib.sha256(files['tests/original.phpt']).hexdigest()}]
            assets = [{'path': name, 'sha256': hashlib.sha256(files[name]).hexdigest()}
                      for name in ['examples/binary.bin', 'examples/empty.txt']]
            with patch.object(phpt, 'COMMIT', pin):
                phpt.prepare(source, tree / 'valid', cases, support_files=assets)
                for item in assets:
                    self.assertEqual((tree / 'valid' / item['path']).read_bytes(), files[item['path']])
                self.assertEqual((tree / 'valid/tests/original.phpt').read_bytes(), files['tests/original.phpt'])
                with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                    phpt.prepare(source, tree / 'bad-hash', cases, support_files=[{**assets[0], 'sha256': '0' * 64}])
                for path in ['../escape', '/absolute', '-ooutput', '.']:
                    with self.assertRaisesRegex(ValueError, 'Unsafe support file'):
                        phpt.prepare(source, tree / 'bad-path', cases, support_files=[{'path': path, 'sha256': '0' * 64}])
                self.assertFalse((tree / 'escape').exists())

    def test_external_network_requires_explicit_offline_override(self):
        with tempfile.TemporaryDirectory() as directory:
            sandbox = Path(directory).resolve()
            engine = Engine('fixture', sandbox / 'unused.so')
            for index, (options, expected, supported) in enumerate([({}, True, False), ({'offline': False}, False, True), ({'offline': False}, False, False)]):
                artifacts = sandbox / str(index)
                (sandbox / 'run-tests.php').write_text("case '--online':\n" if supported else "case '--offline':\n")
                def run(command, **kwargs):
                    self.assertEqual('--offline' in command, expected)
                    self.assertEqual('--online' in command, not expected and supported)
                    if not expected:
                        self.assertEqual(kwargs['env']['SKIP_ONLINE_TESTS'], '0')
                    scan = Path(kwargs['env']['PHP_INI_SCAN_DIR'].split(os.pathsep)[-1])
                    self.assertEqual((scan / 'swoole.ini').read_text(), f'extension={engine.extension.resolve()}\n')
                    (artifacts / 'status.txt').write_text(f'PASSED {sandbox / "original.phpt"}\n')
                    return subprocess.CompletedProcess(command, 0, '', '')
                with patch.object(phpt, 'run_isolated', run):
                    rows, timing = phpt.run_cases(engine, [{'path': 'original.phpt'}],
                                                  sandbox, artifacts, 1, 1, **options)
                self.assertEqual(rows['original.phpt']['status'], 'passed')
                self.assertEqual(timing['offline'], expected)

    def test_child_php_matches_exact_engine_arguments_without_global_path_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            executable = root / "real php 'quoted'"
            executable.write_text('#!' + sys.executable + '\nimport json, sys\nprint(json.dumps(sys.argv[1:]))\n')
            executable.chmod(0o755)
            engine = Engine('fixture', root / "selected 'engine'.so", str(executable), ('sockets', "extension with 'quote'"))
            artifacts = root / 'artifacts'
            original_path = os.environ.get('PATH')
            settings = ('precision=8', 'zend.assertions=-1')
            def run(command, **kwargs):
                self.assertEqual(command[0], str(executable))
                self.assertEqual(os.environ.get('PATH'), original_path)
                result = subprocess.run(['php', '-r', "echo 'literal $value';"], env=kwargs['env'],
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout),
                                 engine.command()[1:] + ['-d', 'precision=8', '-d', 'zend.assertions=-1',
                                                        '-d', 'zend.assertions=1', '-r', "echo 'literal $value';"])
                (artifacts / 'status.txt').write_text('PASSED ' + str(root / 'original.phpt') + '\n')
                return subprocess.CompletedProcess(command, 0, '', '')
            with patch.object(phpt, 'run_isolated', run):
                rows, receipt = phpt.run_cases(engine, [{'path': 'original.phpt'}], root, artifacts, 1, 1,
                                               runner_ini=settings, match_child_php=True)
            self.assertEqual(rows['original.phpt']['status'], 'passed')
            self.assertEqual(os.environ.get('PATH'), original_path)
            selector = receipt['child_php_selector']
            self.assertEqual(selector['sha256'], __import__('hashlib').sha256(Path(selector['path']).read_bytes()).hexdigest())
            self.assertEqual(selector['fallback_exec_argv'][0], str(executable))

    def test_owned_selection_cannot_change_reference_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / 'selection.json'
            manifest.write_text(json.dumps({'commit': 'untrusted-pin', 'tests': [{
                'path': 'tests/swoole_http_server/original.phpt', 'module': 'http'}]}))
            with patch.object(sys, 'argv', ['run.py', '--manifest', str(manifest)]), \
                    patch.object(phpt, 'prepare') as prepare:
                with self.assertRaises(SystemExit) as result:
                    phpt.main()
                self.assertEqual(result.exception.code, 2)
                prepare.assert_not_called()

    def test_only_pure_parser_and_codec_join_the_existing_batch(self):
        pure = ['tests/swoole_http_server/create_request.phpt', 'tests/swoole_websocket_server/pack.phpt']
        isolated = ['tests/swoole_timer/after.phpt', 'tests/swoole_timer/after_fork.phpt',
                    'tests/swoole_timer/reinit_1.phpt', 'tests/swoole_timer/reinit_2.phpt',
                    'tests/swoole_timer/info.phpt', 'tests/swoole_timer/stats.phpt',
                    'tests/swoole_timer/clearAll.phpt', 'tests/swoole_websocket_server/dynamic_property.phpt']
        cases = [{'path': path} for path in pure + isolated]
        self.assertEqual([[case['path'] for case in group] for group in phpt.batches(cases)],
                         [pure, *[[path] for path in isolated]])
        self.assertTrue(all(len(group) == 1 for group in phpt.batches(cases, isolate=True)))

    def test_warned_retry_result_is_failed_even_when_runner_exits_zero(self):
        with tempfile.TemporaryDirectory() as directory:
            sandbox = Path(directory).resolve()
            artifact = sandbox / 'artifacts'
            engine = Engine('fixture', sandbox / 'unused.so')
            def run(command, **kwargs):
                (artifact / 'status.txt').write_text(f'WARNED {sandbox / "retry.phpt"}\n')
                return subprocess.CompletedProcess(command, 0, 'Test passed on retry attempt', '')
            with patch.object(phpt, 'run_isolated', run):
                rows, _ = phpt.run_cases(engine, [{'path': 'retry.phpt'}], sandbox, artifact, 1, 1)
            self.assertEqual(rows['retry.phpt']['status'], 'failed')
            self.assertEqual(rows['retry.phpt']['upstream_status'], 'WARNED')

    def test_failed_checkpoint_keeps_previous_json_and_cleans_temporary_file(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'result.json'
            phpt.save_report(output, {'previous': True})
            def broken_write(path, report):
                Path(path).write_text('{')
                self.assertEqual(json.loads(output.read_text()), {'previous': True})
                raise OSError('fixture write failure')
            with patch.object(phpt, 'write_report', broken_write):
                with self.assertRaisesRegex(OSError, 'fixture write failure'):
                    phpt.save_report(output, {'next': True})
            self.assertEqual(json.loads(output.read_text()), {'previous': True})
            self.assertEqual(list(Path(directory).iterdir()), [output])

    def test_interrupted_run_retains_completed_results_and_unsupported_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'result.json'
            channel, timer, codec = ('tests/swoole_channel_coro/basic.phpt',
                                     'tests/swoole_timer/after.phpt', 'tests/swoole_websocket_server/pack.phpt')
            engine = SimpleNamespace(name='fixture', metadata=lambda: {
                'name': 'fixture', 'capabilities': ['channels', 'coroutines', 'upstream_test_bootstrap', 'event_loop', 'timers']})
            def prepare(source, sandbox, cases, support_files=()):
                (sandbox / 'tests/include').mkdir(parents=True)
                (sandbox / 'tests/include/bootstrap.php').write_text('fixture')
                (sandbox / 'run-tests.php').write_text('fixture')
            def run(engine, group, *args, **kwargs):
                saved = json.loads(output.read_text())
                self.assertEqual(saved['execution_status'], 'running')
                if group[0]['path'] == timer:
                    self.assertEqual({row['test']: row['status'] for row in saved['results']},
                                     {codec: 'unsupported', channel: 'passed'})
                    raise KeyboardInterrupt
                return {channel: {'status': 'passed', 'seconds': 0}}, {'seconds': 0}
            argv = ['run.py', '--engine', 'stub=/stub', '--source', directory, '--output', str(output),
                    '--case', channel, '--case', timer, '--case', codec]
            with patch.object(sys, 'argv', argv), patch.object(phpt, 'prepare', prepare), \
                 patch.object(phpt, 'parse_engines', return_value=[engine]), \
                 patch.object(phpt.subprocess, 'check_output', return_value='fixture-tree'), \
                 patch.object(phpt, 'run_cases', run):
                with self.assertRaises(KeyboardInterrupt):
                    phpt.main()
            saved = json.loads(output.read_text())
            self.assertEqual(saved['execution_status'], 'interrupted')
            self.assertEqual({row['test']: row['status'] for row in saved['results']},
                             {codec: 'unsupported', channel: 'passed'})

    def test_missing_duplicate_and_foreign_ledger_results_fail_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            sandbox = Path(directory).resolve()
            artifacts = sandbox / 'artifacts'
            engine = Engine('unused', sandbox / 'unused.so')
            path = str(sandbox / 'one.phpt')
            for ledger in ['', f'PASSED {path}\nPASSED {path}\n', f'PASSED {path}\nPASSED /foreign.phpt\n']:
                def fake_run(command, **kwargs):
                    (artifacts / 'status.txt').write_text(ledger)
                    return subprocess.CompletedProcess(command, 0, '', '')
                with patch.object(phpt, 'run_isolated', fake_run):
                    rows, _ = phpt.run_cases(engine, [{'path': 'one.phpt'}], sandbox, artifacts, 1, 1)
                self.assertEqual(rows['one.phpt']['status'], 'failed')

    def test_only_audited_cases_batch(self):
        safe = {'path': next(iter(phpt.PARALLEL_SAFE))}
        network = {'path': 'tests/swoole_websocket_server/ping.phpt'}
        future = {'path': 'tests/swoole_table/future.phpt'}
        self.assertEqual(phpt.batches([safe, network, future]), [[safe], [network], [future]])
        self.assertEqual(phpt.batches([safe, safe], isolate=True), [[safe], [safe]])

    @unittest.skipUnless((ROOT / 'target/upstream/src/modules/swoole.so').exists(), 'Built reference required')
    def test_official_parallel_runner_preserves_failure_skip_and_xfail(self):
        engine = Engine('upstream', ROOT / 'target/upstream/src/modules/swoole.so')
        with tempfile.TemporaryDirectory() as directory:
            sandbox = Path(directory)
            shutil.copy2(ROOT / 'target/upstream/src/run-tests.php', sandbox / 'run-tests.php')
            for name in ['home', 'tmp']:
                (sandbox / name).mkdir()
            cases = []
            for name, extra, expected in [('pass', '', 'actual'), ('fail', '', 'wrong'),
                                           ('skip', '--SKIPIF--\n<?php die("skip fixture"); ?>\n', 'actual'),
                                           ('xfail', '--XFAIL--\nKnown fixture defect\n', 'wrong')]:
                path = name + '.phpt'
                (sandbox / path).write_text(f'--TEST--\n{name}\n{extra}--FILE--\n<?php echo "actual"; ?>\n--EXPECT--\n{expected}\n')
                cases.append({'path': path})
            rows, timing = phpt.run_cases(engine, cases, sandbox, sandbox / 'artifacts', 5, 2)
            self.assertEqual({name: row['status'] for name, row in rows.items()},
                             {'pass.phpt': 'passed', 'fail.phpt': 'failed', 'skip.phpt': 'skipped', 'xfail.phpt': 'failed'})
            self.assertNotEqual(timing['exit_code'], 0)
            self.assertTrue((Path(rows['fail.phpt']['artifacts']) / 'fail.diff').exists())


class Cleanup(unittest.TestCase):
    def test_permission_error_preserves_exited_failure_and_output(self):
        from tools import harness
        from unittest.mock import Mock
        process = Mock(returncode=86)
        process.communicate.return_value = ('diagnostic', 'ASan failure')
        process.poll.return_value = 86
        with patch.object(harness.subprocess, 'Popen', return_value=process), \
             patch.object(harness, 'stop_process_group', side_effect=PermissionError('denied')):
            result = harness.run_isolated(['diagnostic'])
        self.assertEqual((result.returncode, result.stdout), (86, 'diagnostic'))
        self.assertIn('ASan failure', result.stderr)
        self.assertIn('cleanup failed', result.stderr)

    def test_uncertain_cleanup_never_turns_success_green(self):
        from tools import harness
        from unittest.mock import Mock
        process = Mock(returncode=0)
        process.communicate.return_value = ('ok', '')
        process.poll.return_value = 0
        with patch.object(harness.subprocess, 'Popen', return_value=process), \
             patch.object(harness, 'stop_process_group', side_effect=PermissionError('denied')):
            self.assertEqual(harness.run_isolated(['diagnostic']).returncode, 125)


class Lanes(unittest.TestCase):
    def test_help_names_the_docker_wrapper(self):
        # AGENTS.md sends Linux-only phases through `dev.py docker`; argparse never sees it.
        result = subprocess.run([sys.executable, ROOT / 'tools/dev.py', '--help'], capture_output=True, text=True)
        self.assertIn('dev.py docker [-- dev.py arguments | -- SCRIPT.py arguments]', ' '.join(result.stdout.split()))

    def test_linux_consumer_hooks_load_curl_once_unless_compiled_in(self):
        for explicit, compiled_in, expected in [([], '', 1), (['curl'], '', 1), ([], 'Core\ncurl\n', 0)]:
            args = SimpleNamespace(lane='integrate', module=['phpcollections', 'base64', 'phphelpers'], all_modules=False,
                                   php='php', php_extension=explicit, jobs=2, test=None, case=None,
                                   benchmark=False, compare=False, fix_halfclose=False, stdext=True)
            with patch.object(dev.sys, 'platform', 'linux'), patch.object(dev.shutil, 'which', return_value='/php'), \
                    patch.object(dev.subprocess, 'run', return_value=SimpleNamespace(stdout=compiled_in)):
                commands = dict(dev.plan(args, ROOT / 'target/reports/collections'))
            for phase in ['phpcollections-runtime',
                          'base64-consumers-upstream', 'base64-consumers-cargo', 'phphelpers-module']:
                loaded = [commands[phase][i+1] for i, argument in enumerate(commands[phase])
                          if argument == '--php-extension']
                self.assertEqual(loaded.count('curl'), expected)

    def test_child_php_selection_is_explicit_and_scoped_to_direct_official_runner(self):
        args = SimpleNamespace(lane='integrate', module=None, all_modules=True, php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False, fix_halfclose=True)
        with patch.object(dev.subprocess, 'check_output', return_value='/openssl'):
            baseline = dict(dev.plan(args, ROOT/'target/reports/child-default'))
            args.match_child_php = True
            matched = dict(dev.plan(args, ROOT/'target/reports/child-selected'))
        self.assertEqual(set(baseline), set(matched))
        direct = 0
        for name, command in baseline.items():
            self.assertNotIn('--match-child-php', command)
            expected = 'corpus/upstream/run.py' in command
            self.assertEqual('--match-child-php' in matched[name], expected, name)
            if expected:
                direct += 1
                self.assertIn('--require-all', matched[name])
        self.assertGreater(direct, 0)
        self.assertNotIn('--match-child-php', matched['cargo-build'])

    def test_all_registered_modules_share_one_plan_and_close_gate(self):
        args = SimpleNamespace(lane='integrate', module=None, all_modules=True, php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False, fix_halfclose=False)
        with patch.object(dev.subprocess, 'check_output', return_value='/openssl'):
            commands = dict(dev.plan(args, ROOT / 'target/reports/all-registered'))
            # phphelpers' runtime fixtures call stdext helpers, so only stdext plans run them.
            stdext = dict(dev.plan(SimpleNamespace(**vars(args), stdext=True), ROOT / 'target/reports/all-registered-stdext'))
        self.assertNotIn('phphelpers-runtime', commands)
        self.assertIn('corebase-native',commands)
        self.assertIn('ossupport-async-file-owner', commands)
        self.assertIn('http-rewrites', commands)
        self.assertIn('swoole-rs-coroutine', commands['coroutine-rust-asan'])
        self.assertIn('swoole-rs-memory', commands['memory-rust-asan'])
        self.assertFalse(any(name.startswith('coroutine-registry') for name in commands))
        for name in ['reference-identity','base64-consumers-upstream','base64-consumers-cargo',
                     'crc32-consumers-upstream','crc32-consumers-cargo',
                     'corebase-startup','corebase-record-views']:
            self.assertIn(name,commands)
        self.assertIn('--build-manifest', commands['corebase-startup'])
        self.assertNotIn('coroutine-native-originals', commands)
        # Upstream PHPTs run in the merge tier's phpt-* jobs, not as per-module phases (#83).
        self.assertNotIn('phpt', commands)
        self.assertEqual([name for name in commands if name.endswith(('-full-originals', '-originals-manifest'))], [])
        self.assertIn('websocket-close',commands)
        self.assertIn('tools/build_upstream.py', commands['reference-build'])
        self.assertIn('--reference', commands['abi'])
        self.assertNotIn('--startup-only', commands['abi'])
        # Nothing uses the hybrid's rust backend (#2199, #2415): the native gates link the Cargo build's SDK.
        self.assertTrue(commands['abi'][commands['abi'].index('--extension') + 1].endswith('target/ext/swoole.so'))
        self.assertIn('--native', commands['cargo-build'])
        self.assertNotIn('core_check', ' '.join(commands['error-native']))
        self.assertIn('--sanitize', commands['buffer-posix-iovec'])
        self.assertIn('--sanitize', commands['ports-callback-property'])
        for family in ['logger', 'hash', 'global-logger', 'redirect']:
            command = commands['utilities-' + family + '-originals']
            self.assertIn('--' + family + '-originals', command)
            self.assertIn('cargo=' + str(ROOT / 'target/ext/swoole.so'), command)
            self.assertTrue(command[command.index('--build-manifest') + 1].endswith('target/ext/build-manifest.json'))
        for family in ['logger', 'hash']:
            command = commands['utilities-' + family + '-originals']
            self.assertIn('--' + family + '-originals', command)
            self.assertEqual(command.count('--engine'), 2)
        self.assertIn('--require-same-api', commands['api-surface'])
        self.assertEqual(commands['api-surface'].count('--engine'), 2)
        self.assertLess(list(commands).index('cargo-build'), list(commands).index('api-surface'))
        self.assertIn('tests/modules/buffer/string/test_gate.py', commands['string-originals'])
        self.assertIn('column-bytes', commands['phpcollections-column-bytes'])
        self.assertEqual(commands['phpcollections-column-bytes'].count('--engine'), 2)
        self.assertIn('--build-manifest', commands['utilities-public-storage'])
        self.assertIn('--archive', commands['utilities-public-storage'])
        self.assertEqual(commands['utilities-public-storage'].count('--engine'), 2)
        self.assertIn('utilities-php-logger',commands)
        # Attested gates check the Cargo build's manifest, not the hybrid's (#2199).
        for name in ['utilities-php-logger', 'phpcollections-runtime', 'ports-configuration', 'clients-php-http']:
            command = commands[name]
            self.assertTrue(command[command.index('--build-manifest') + 1].endswith('target/ext/build-manifest.json'))
            self.assertNotIn('--archive', command)
        self.assertNotIn('httpbindings-php', commands)
        self.assertIn('--archive', commands['ports-callback-property'])
        self.assertIn('--sanitize', commands['ports-callback-property'])
        self.assertNotIn('ports-php', commands)
        self.assertNotIn('ports-callback-php', commands)
        self.assertNotIn('--case', commands['ports-configuration'])
        self.assertIn('--build-manifest', stdext['phphelpers-runtime'])
        self.assertEqual(stdext['phphelpers-runtime'].count('--engine'), 2)
        self.assertNotIn('phphelpers-policy', commands)
        self.assertNotIn('phphelpers-async', commands)  # Its cases run in phphelpers-unit.
        self.assertIn('--archive', commands['ports-policy'])
        self.assertIn('--archive', commands['clients-http-format'])
        self.assertIn('--sanitize', commands['clients-http-format'])
        self.assertNotIn('clients-pool', commands)  # Its cases run in clients-unit.
        for family in ['http', 'dns', 'lifecycle', 'pool', 'resolver', 'pool_fork', 'http_connection', 'http_format', 'coro', 'async']:
            command = commands['clients-php-' + family]
            self.assertIn('--build-manifest', command)
            self.assertEqual(command.count('--engine'), 2)
        self.assertNotIn('clients-policy', commands)  # Its cases run in clients-unit.
        self.assertNotIn('clients-ip', commands)  # So do these.
        self.assertNotIn('clients-codec', commands)  # Its cases run in clients-unit.
        self.assertNotIn('clients-ip-text', commands)  # So do these.
        self.assertIn('--archive',commands['http-locales'])
        self.assertIn('--archive',commands['http-rewrites'])
        self.assertIn('timer-bailout',commands)
        self.assertFalse(any(name.startswith('halfclose-') for name in commands))

    def test_stdext_changes_actual_configure(self):
        args = SimpleNamespace(jobs=2, php_config='php-config', openssl_dir='/fixture/openssl', stdext=False)
        with patch.object(build_upstream.subprocess, 'check_output', return_value='-I/fixture/pcre'):
            disabled, _ = build_upstream.extension_configuration(args)
            args.stdext = True
            enabled, _ = build_upstream.extension_configuration(args)
        self.assertNotIn('--enable-swoole-stdext', disabled)
        self.assertIn('--enable-swoole-stdext', enabled)

    def test_php_sockets_configuration_is_explicit_and_not_inherited(self):
        args = SimpleNamespace(jobs=2, php_config='php-config', openssl_dir='/fixture/openssl')
        with patch.dict(os.environ, {'PHP_SOCKETS': 'yes'}), patch.object(
                build_upstream.subprocess, 'check_output', return_value='-I/fixture/pcre'):
            disabled, _ = build_upstream.extension_configuration(args)
            args.php_sockets = True
            enabled, _ = build_upstream.extension_configuration(args)
        self.assertIn('--disable-sockets', disabled)
        self.assertNotIn('--enable-sockets', disabled)
        self.assertIn('--enable-sockets', enabled)

    def test_php_sockets_selects_both_profiles_without_enabling_storage_in_native(self):
        from itertools import product
        for sockets, stdext in product([False, True], repeat=2):
            args = SimpleNamespace(lane='integrate', all_modules=True, module=None, php='php',
                                   php_extension=[], jobs=2, test=None, case=None, benchmark=False,
                                   compare=False, fix_halfclose=False, php_sockets=sockets,
                                   stdext=stdext)
            commands = dict(dev.plan(args, ROOT / 'target/reports/php-sockets'))
            native = commands['stdext-reference-build' if stdext else 'reference-build']
            for command in [commands['cargo-build'], native]:
                self.assertEqual('--php-sockets' in command, sockets)
            self.assertNotIn('--module', native)
            suffix = ('-php-sockets' if sockets else '') + ('-stdext' if stdext else '')
            tree = ROOT / ('target/upstream-builds/' + suffix[1:] + '/src' if suffix else 'target/upstream/src')
            self.assertIn('upstream=' + str(tree / 'modules/swoole.so'),
                          commands['api-surface'])
            self.assertEqual('php-sockets-sendfile-original' in commands, sockets)
            if sockets:
                original = commands['php-sockets-sendfile-original']
                self.assertIn('tests/swoole_server/sendfile_02.phpt', original)
                self.assertIn('--require-all', original)
                self.assertIn('--isolate-cases', original)

    def test_stdext_uses_separate_real_native_and_rust_profiles(self):
        args = SimpleNamespace(lane='integrate', module=['phphelpers'], stdext=True, php='php',
                               php_extension=[], jobs=2, test=None, case=None, benchmark=False,
                               compare=False, fix_halfclose=False)
        commands = dict(dev.plan(args, ROOT / 'target/reports/stdext-profiles'))
        self.assertIn('--stdext', commands['cargo-build'])
        native = commands['stdext-reference-build']
        self.assertIn('tools/build_upstream.py', native)
        self.assertIn('--stdext', native)
        self.assertNotIn('--module', native)
        self.assertLess(list(commands).index('stdext-reference-build'), list(commands).index('cargo-build'))
        engines = [command for command in commands.values() if any(str(a).startswith('upstream=') for a in command)]
        self.assertTrue(engines)
        # Upstream engines come from the stdext reference build.
        for command in engines:
            self.assertIn('upstream=' + str(ROOT / 'target/upstream-builds/stdext/src/modules/swoole.so'), map(str, command))

    def test_halfclose_repair_is_separate_from_rust_features_and_aliases(self):
        args = SimpleNamespace(lane='integrate', module=['error'], php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False,
                               fix_halfclose=True)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-repair'))
        self.assertIn('--fix-halfclose', commands['cargo-build'])
        self.assertEqual(commands['error-php'].count('--engine'), 2)
        self.assertEqual(commands['halfclose-prequeue'].count('--engine'), 1)
        self.assertNotIn('--probe-coroutines', commands['halfclose-prequeue'])
        self.assertIn('--keep-alive', commands['halfclose-pressure'])
        self.assertNotIn('halfclose-stale-read', commands)  # a crates/server unit test

    def test_protocol_lifetime_gates_use_actual_reference_first(self):
        args = SimpleNamespace(lane='integrate', module=['protocols'], php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False,
                               fix_halfclose=False)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-protocol-lifetimes'))
        for name in ['reply', 'request', 'handlers', 'bailout', 'lifecycle']:
            command = commands['protocols-' + name]
            first = command.index('--engine')
            self.assertTrue(command[first + 1].startswith('upstream='))
            self.assertEqual(command.count('--engine'), 2)
        self.assertFalse(any(name.startswith('halfclose-') for name in commands))

    def test_native_halfclose_keeps_all_pause_profiles_and_originals(self):
        args = SimpleNamespace(lane='integrate', module=['error'], php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False,
                               fix_halfclose=True)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-paused-tls'))
        for suffix in ['', '-coroutines']:
            prefix = 'halfclose-paused' + suffix
            self.assertEqual('--coroutines' in commands[prefix], bool(suffix))
            for version in ['1.2', '1.3']:
                command = commands[prefix + '-tls' + version]
                self.assertEqual(command[command.index('--tls-version') + 1], version)
                self.assertIn('--tls-cert', command)
        self.assertIn('--require-all', commands['halfclose-pause-originals'])

    def test_explicit_tls13_python_changes_only_tls13_client_profiles(self):
        args = SimpleNamespace(lane='integrate', module=['error'], php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False,
                               fix_halfclose=True, tls13_python=Path('/private/python-with-tls13'))
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-explicit-tls-provider'))
        self.assertEqual(commands['halfclose-tls13-driver'][0], str(args.tls13_python))
        for suffix in ['', '-coroutines']:
            prefix = 'halfclose-paused' + suffix
            self.assertEqual(commands[prefix][0], sys.executable)
            for version in ['1.2', '1.3']:
                self.assertEqual(commands[prefix + '-tls' + version][0],
                                 str(args.tls13_python) if version == '1.3' else sys.executable)

    def test_candidate_phpt_families_come_from_the_touched_modules_only(self):
        # Consumers run their gates, not their PHPT families: those overran the phase cap (#2270).
        from argparse import Namespace
        base = dict(lane='candidate', module=['ports', 'process'], php='php', php_extension=[],
                    jobs=4, test=None, case=None, benchmark=False, compare=False)
        families = lambda touched: [arg for arg in dict(dev.plan(Namespace(phpt_module=touched, **base),
                                    ROOT / 'target/reports/test-plan')).get('phpt', []) if str(arg).startswith('swoole_')]
        self.assertEqual(families([]), [])
        self.assertIn('swoole_client_coro', families(['clients']))
        self.assertNotIn('swoole_server', families(['clients']))
        self.assertIn('swoole_server', families(None))

    def test_integration_cannot_filter_or_use_development_build(self):
        from argparse import Namespace
        args = Namespace(lane='integrate', module=['timer'], php='php', php_extension=[],
                         jobs=4, test=None, case=None, benchmark=False, compare=False)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-plan'))
        self.assertNotIn('phpt', commands)  # the merge tier's phpt-* jobs run every PHPT
        self.assertIn('--build-manifest', commands['timer-native'])


class ChangedModules(unittest.TestCase):
    def changed(self, *paths):
        with patch.object(dev.subprocess, 'check_output', return_value='\n'.join(paths)):
            return dev.changed_modules('origin/main')

    def test_owned_paths_select_their_module_and_shared_inputs_select_all(self):
        self.assertEqual(self.changed('crates/timer/src/lib.rs', 'tests/modules/base64/README.md', 'docs/x.md'),
                         ['base64', 'timer'])
        self.assertEqual(self.changed('.github/workflows/pr.yml', 'tests/dev/test_tooling.py'), [])
        self.assertEqual(self.changed('crates/ffi/src/lib.rs'), sorted(dev.MODULES))
        self.assertEqual(self.changed('crates/oracle/build.rs', 'fuzz/src/lib.rs'), [])
        self.assertEqual(self.changed('native/halfclose/contract.md'), sorted(dev.MODULES))
        # The Cargo build every gate and PHPT job loads.
        for path in ('crates/ext/build.rs', 'tools/build_ext.py', 'rust-toolchain.toml', '.cargo/config.toml'):
            self.assertEqual(self.changed(path), sorted(dev.MODULES), path)
        self.assertEqual(self.changed('crates/sqlite/src/lib.rs', 'crates/oci/src/lib.rs'), ['hooks'])

    def test_every_crate_has_an_owner(self):
        owned = [path for module in json.loads((dev.ROOT / 'migration/modules.json').read_text())['modules']
                 for path in module['owned_paths']]
        for crate in sorted((dev.ROOT / 'crates').glob('*/Cargo.toml')):
            directory = f'crates/{crate.parent.name}/'
            self.assertTrue(directory in owned or directory.startswith(dev.SHARED_PATHS), directory)

    @unittest.skipUnless((ROOT / 'target/upstream/src/tests').is_dir(), 'Pinned checkout required')
    def test_every_upstream_phpt_family_has_an_owner(self):
        # Else a PR touching the code a family tests runs none of it; the runner's own test has no module.
        owned = {family for module in json.loads((ROOT / 'migration/modules.json').read_text())['modules']
                 for family in module.get('phpt_families', [])}
        tests = ROOT / 'target/upstream/src/tests'
        families = {path.relative_to(tests).parts[0] for path in tests.rglob('*.phpt')}
        self.assertEqual(families - owned - {'swoole_test_runner'}, set())

    def test_a_changed_crate_selects_the_modules_whose_packages_depend_on_it(self):
        # Their gates call its exports, so they catch one removed or renamed (#2236).
        with patch.object(dev, 'dependents', return_value={'crates/base64/': {'crates/http/', 'tests/modules/websocket/'}}):
            self.assertEqual(self.changed('crates/base64/src/lib.rs'), ['base64', 'http', 'websocket'])
            # The PHPT families come from the touched modules alone.
            with patch.object(dev.subprocess, 'check_output', return_value='crates/base64/src/lib.rs'):
                self.assertEqual(dev.changed_modules('origin/main', consumers=False), ['base64'])
        self.assertIn('crates/http/', dev.dependents()['crates/base64/'])


class FocusedPhases(unittest.TestCase):
    def test_skipped_phases_are_not_run_and_the_run_is_partial(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            planned = [('ownership', ['check-ownership']), ('phpt', ['families.sh'])]
            with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                 patch.object(dev, 'execute', return_value=SimpleNamespace(returncode=0)) as run, \
                 patch('sys.argv', ['dev.py', 'candidate', '--module', 'locks', '--skip-phase', 'php*']):
                self.assertEqual(dev.main(), 0)
            report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
            self.assertEqual((report['status'], report['selected_phases']), ('partial_passed', ['ownership']))
            self.assertEqual(report['not_run'][0]['phase'], 'phpt')
            self.assertEqual(run.call_count, 1)

    def test_explicit_selection_keeps_canonical_order_and_records_omitted_gates(self):
        for codes, status, exit_code in [([0, 0], 'partial_passed', 0), ([0, 2], 'partial_failed', 1)]:
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                planned = [('ownership', ['check-ownership']), ('api', ['check-api']), ('originals', ['check-originals'])]
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                     patch.object(dev, 'execute', side_effect=[SimpleNamespace(returncode=code) for code in codes]) as run, \
                     patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--phase', 'api', '--phase', 'ownership', '--phase', 'api']):
                    self.assertEqual(dev.main(), exit_code)
                report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
                self.assertEqual(report['status'], status)
                self.assertFalse(report['full_plan_passed'])
                self.assertEqual(report['planned_phases'], ['ownership', 'api', 'originals'])
                self.assertEqual(report['selected_phases'], ['ownership', 'api'])
                self.assertEqual([call.args[0] for call in run.call_args_list], [['check-ownership'], ['check-api']])
                self.assertEqual(report['not_run'], [{'phase': 'originals', 'status': 'not_run', 'reason': 'explicit_phase_selection'}])
                self.assertEqual(report['phases'][-1]['returncode'], codes[-1])

    def test_failed_ownership_or_producer_stops_before_using_old_aliases(self):
        for phase in ['ownership', 'cargo-build', 'reference-build', 'stdext-reference-build']:
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                planned = [(phase, ['failed-prerequisite']), ('api', ['stale-alias']), ('originals', ['old-engine'])]
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                     patch.object(dev, 'require_build_space'), \
                     patch.object(dev, 'execute', return_value=SimpleNamespace(returncode=2)) as run, \
                     patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks']):
                    self.assertEqual(dev.main(), 1)
                run.assert_called_once()
                self.assertEqual(run.call_args.args[0], ['failed-prerequisite'])
                report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
                self.assertEqual(report['status'], 'failed')
                self.assertEqual(report['blocked_phase'], phase)
                self.assertEqual(report['phases'][0]['returncode'], 2)
                self.assertFalse(report['full_plan_passed'])
                self.assertEqual([row['phase'] for row in report['not_run']], ['api', 'originals'])
                self.assertTrue(all(row['status'] == 'not_run' for row in report['not_run']))

    def test_failed_phases_are_listed_with_logs_and_in_the_step_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); summary = root / 'summary.md'
            planned = [('first', ['fixture']), ('second', ['fixture']), ('third', ['fixture'])]
            results = iter([1, 0, 2])
            with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                 patch.object(dev, 'execute', side_effect=lambda *a, **k: SimpleNamespace(returncode=next(results))), \
                 patch.dict(os.environ, {'GITHUB_STEP_SUMMARY': str(summary)}), \
                 patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--jobs', '1']), \
                 patch('sys.stdout', new_callable=__import__('io').StringIO) as stdout:
                self.assertEqual(dev.main(), 1)
            for text in [stdout.getvalue(), summary.read_text()]:
                self.assertIn('2 of 3 phases failed:', text)
                self.assertIn('- first (exit 1): ' + str(next(root.glob('target/reports/dev/*')) / 'first.log'), text)
                self.assertIn('- third (exit 2): ', text)
                self.assertNotIn('- second', text)

    def test_child_disk_refusal_stops_but_behavior_failure_continues(self):
        for error, details, expected_calls in [('Build disk space low: fixture needs 4GiB', {}, 1),
                ('Unchanged 4GiB guard', dict(minimum_free_bytes=4294967296, blocked='/private/scratch'), 1),
                ('Unchanged4GiBguard', dict(minimum_free_bytes=4294967296, blocked=dict(path='/scratch')), 1),
                ('Compiler refused by unchanged 4GiB guard', dict(commands=[dict(
                    blocked='unchanged 4GiB guard', free_bytes={'/scratch': 4294967295})]), 1),
                ('Compiler refused by unchanged 4GiB guard', dict(commands=[dict(
                    blocked='unchanged 4GiB guard', free_bytes={'/scratch': 4294967296})]), 2),
                ('Compiler refused by unchanged 4GiB guard', dict(commands=[dict(
                    blocked='unchanged 4GiB guard', free_bytes={'/scratch': 'low'})]), 2),
                ('Compiler refused by unchanged 4GiB guard', dict(commands=None), 2),
                ('Unchanged4GiBguard', {}, 2), ('Behavior comparison failed', {}, 2)]:
            with self.subTest(error=error), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); output = root / 'gate.json'
                output.write_text(json.dumps(dict(passed=False, error=error, **details)))
                planned = [('module-sdk', ['fixture', '--report', str(output)]), ('originals', ['later'])]
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                     patch.object(dev, 'execute', return_value=SimpleNamespace(returncode=1)) as run, \
                     patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--jobs', '1']):
                    self.assertEqual(dev.main(), 1)
                report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
                self.assertEqual(run.call_count, expected_calls)
                self.assertEqual(report['status'], 'failed')
                self.assertFalse(report['full_plan_passed'])
                self.assertTrue(report['phases'][0]['failed'])
                self.assertEqual(report['phases'][0]['outcome_sha256'],
                                 __import__('hashlib').sha256(output.read_bytes()).hexdigest())
                if expected_calls == 1:
                    self.assertEqual(report['infrastructure_error'], error)
                    self.assertEqual(report['blocked_phase'], 'module-sdk')
                    self.assertEqual(report['not_run'], [dict(phase='originals', status='not_run',
                                                             reason='run_stopped_before_phase')])
                else:
                    self.assertNotIn('infrastructure_error', report)
                    self.assertEqual(report['not_run'], [])

    def test_zero_exit_cannot_hide_failed_missing_or_malformed_gate_receipts(self):
        cases = [('false', {'passed': False}, True), ('positive-subset', {'passed': True, 'full_gate_passed': False}, False),
                 ('manifest', {'tests': []}, False), ('invalid-flag', {'passed': 'yes'}, True),
                 ('missing', None, True), ('malformed', '{', True)]
        for label, payload, failed, flag in [(label, payload, failed, flag) for label, payload, failed in cases for flag in ('--output', '--report')]:
            with self.subTest(label=label, flag=flag), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); output = root/'gate.json'
                if payload is not None:
                    output.write_text(payload if isinstance(payload, str) else json.dumps(payload))
                planned = [('ownership', ['fixture', flag, str(output)]), ('originals', ['later'])]
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                     patch.object(dev, 'execute', return_value=SimpleNamespace(returncode=0)), \
                     patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--phase', 'ownership']):
                    self.assertEqual(dev.main(), int(failed))
                report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
                self.assertEqual(report['status'], 'partial_failed' if failed else 'partial_passed')
                self.assertFalse(report['full_plan_passed'])
                self.assertEqual(report['phases'][0]['returncode'], 0)
                self.assertEqual(report['phases'][0]['failed'], failed)
                self.assertEqual(report['not_run'][0]['phase'], 'originals')
                if isinstance(payload, dict) and isinstance(payload.get('passed'), bool):
                    self.assertEqual(report['phases'][0]['outcome_passed'], payload['passed'])
                    self.assertEqual(report['phases'][0]['outcome_sha256'], __import__('hashlib').sha256(output.read_bytes()).hexdigest())

    def test_unknown_or_benchmark_phase_selection_fails_before_execution(self):
        import contextlib
        import io
        for extra in [['--phase', 'missing'], ['--phase', 'ownership', '--benchmark']]:
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=[('ownership', ['fixture'])]), \
                     patch.object(dev, 'execute') as run, patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', *extra]), \
                     contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                    dev.main()
                run.assert_not_called()
                self.assertFalse((root / 'target').exists())

    def test_phase_patterns_select_every_matching_phase(self):
        import contextlib
        import io
        planned = [('halfclose-a', ['a']), ('other', ['o']), ('halfclose-b', ['b'])]
        output = io.StringIO()
        with patch.object(dev, 'plan', return_value=planned), contextlib.redirect_stdout(output), \
             patch('sys.argv', ['dev.py', 'integrate', '--phase', 'halfclose-*', '--plan']):
            dev.main()
        self.assertEqual([row['phase'] for row in json.loads(output.getvalue())], ['halfclose-a', 'halfclose-b'])

    def test_selecting_every_phase_still_does_not_claim_complete_plan(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=[('ownership', ['fixture'])]), \
                 patch.object(dev, 'execute', return_value=SimpleNamespace(returncode=0)), \
                 patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--phase', 'ownership']):
                self.assertEqual(dev.main(), 0)
            report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
            self.assertEqual(report['status'], 'partial_passed')
            self.assertFalse(report['full_plan_passed'])
            self.assertEqual(report['not_run'], [])


if __name__ == '__main__':
    unittest.main(verbosity=2)
