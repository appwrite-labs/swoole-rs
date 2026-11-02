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
from tools import build_hybrid, dev, harness
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


class RustCache(unittest.TestCase):
    def test_cargo_artifact_selects_actual_archive_and_rejects_failed_or_ambiguous_build(self):
        artifact = dict(reason='compiler-artifact', target=dict(name='swoole_rs_ffi'),
                        filenames=['/explicit/private-target/release/libswoole_rs_ffi.a'])
        result = subprocess.CompletedProcess(['cargo'], 0, json.dumps(artifact) + '\n')
        with patch.object(build_hybrid.subprocess, 'run', return_value=result):
            archive, receipt = build_hybrid.rust_archive(['httpbindings'])
        self.assertEqual(archive, Path(artifact['filenames'][0]))
        self.assertIn('--features', receipt['command'])
        for result in [subprocess.CompletedProcess(['cargo'], 1, ''),
                       subprocess.CompletedProcess(['cargo'], 0, ''),
                       subprocess.CompletedProcess(['cargo'], 0, (json.dumps(artifact) + '\n') * 2)]:
            with patch.object(build_hybrid.subprocess, 'run', return_value=result):
                with self.assertRaises((ValueError, subprocess.CalledProcessError)):
                    build_hybrid.rust_archive([])

    def test_selected_source_attestation_follows_build_dependencies(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            for name in ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml']:
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text(name)
            packages, nodes = [], []
            for name, dependencies in [('ffi', [('selected', None), ('dev', 'dev')]),
                                       ('selected', [('shared', 'build')]), ('shared', []), ('dev', []), ('inactive', [])]:
                crate = root / 'crates' / name; (crate / 'src').mkdir(parents=True)
                (crate / 'Cargo.toml').write_text(name); (crate / 'src/lib.rs').write_text(name)
                packages.append(dict(id=name, source=None, manifest_path=str(crate / 'Cargo.toml'), targets=[]))
                nodes.append(dict(id=name, deps=[dict(pkg=dep, dep_kinds=[dict(kind=kind)]) for dep, kind in dependencies]))
            metadata = dict(packages=packages, resolve=dict(root='ffi', nodes=nodes))
            with patch.object(build_hybrid, 'ROOT', root), patch.object(build_hybrid.subprocess, 'check_output', return_value=json.dumps(metadata)):
                first = build_hybrid.rust_sources(['selected'])
                self.assertIn('crates/shared/src/lib.rs', first)
                self.assertIn('.cargo/config.toml', first)
                self.assertNotIn('crates/dev/src/lib.rs', first)
                self.assertNotIn('crates/inactive/src/lib.rs', first)
                (root / 'crates/inactive/src/lib.rs').write_text('unrelated')
                self.assertEqual(first, build_hybrid.rust_sources(['selected']))
                (root / 'crates/shared/src/lib.rs').write_text('changed dependency')
                second = build_hybrid.rust_sources(['selected'])
                self.assertNotEqual(first, second)
                before = {'rust_sources_sha256': first, 'patches': [], 'headers_sha256': {}}
                after = {**before, 'rust_sources_sha256': second}
                self.assertEqual(build_hybrid.configuration_key(before, True), build_hybrid.configuration_key(after, True))
                self.assertNotEqual(build_hybrid.configuration_key(before), build_hybrid.configuration_key(after))

class Overlays(unittest.TestCase):
    def test_cached_export_rebuilds_unknown_or_old_producer_without_relabeling(self):
        inputs = {'upstream_commit': 'pinned', 'configure': ['--enable-sockets']}
        for producer in [None, 'old-source', 'current-source']:
            record = {'inputs': inputs.copy()}
            if producer is not None:
                record['producer_source_ref'] = producer
            before = json.dumps(record, sort_keys=True)
            self.assertEqual(build_hybrid.cached_build_matches(record, inputs, 'current-source'),
                             producer == 'current-source')
            self.assertTrue(build_hybrid.cached_build_matches(record, inputs))
            self.assertFalse(build_hybrid.cached_build_matches(record, {**inputs, 'configure': []}, 'current-source'))
            self.assertEqual(json.dumps(record, sort_keys=True), before)

    def test_only_selected_contracts_are_staged_and_collisions_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['include', 'websocket', 'buffer']:
                (root / 'native' / name).mkdir(parents=True)
            for name in ['include/swoole_rs.h', 'include/swoole_rs_websocket.h',
                         'websocket/codec.h', 'buffer/string.h']:
                (root / 'native' / name).touch()
            with patch.object(build_hybrid, 'ROOT', root):
                selected = build_hybrid.headers_for(['websocket'])
                self.assertEqual({p.name for p in selected}, {'swoole_rs.h', 'swoole_rs_websocket.h', 'codec.h'})
                (root / 'native/websocket/swoole_rs.h').touch()
                with self.assertRaises(ValueError):
                    build_hybrid.headers_for(['websocket'])

    def test_ports_only_stages_real_legacy_request_view_without_http_owner_headers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ['include/swoole_rs.h', 'ports/ports.h', 'http/swoole_rs_http_request_view.h',
                         'http/swoole_rs_http_request_v2.h', 'http/swoole_rs_http.h']:
                path = root / 'native' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
            with patch.object(build_hybrid, 'ROOT', root):
                ports = {p.name for p in build_hybrid.headers_for(['ports'])}
                self.assertEqual(ports, {'swoole_rs.h', 'ports.h', 'swoole_rs_http_request_view.h'})
                paired = build_hybrid.headers_for(['ports', 'http'])
                self.assertEqual(sum(p.name == 'swoole_rs_http_request_view.h' for p in paired), 1)
                self.assertIn('swoole_rs_http_request_v2.h', {p.name for p in paired})

    def test_development_configuration_invalidation(self):
        inputs = {'patches': [{'upstream_paths': ['src/core/buffer.cc'], 'sha256': 'one'}],
                  'rust_archive_sha256': 'one', 'headers_sha256': {'abi.h': 'one'}, 'modules': ['buffer']}
        native_edit = {**inputs, 'rust_archive_sha256': 'two',
                       'patches': [{'upstream_paths': ['src/core/buffer.cc'], 'sha256': 'two'}]}
        self.assertEqual(build_hybrid.configuration_key(inputs, True), build_hybrid.configuration_key(native_edit, True))
        self.assertNotEqual(build_hybrid.configuration_key(inputs), build_hybrid.configuration_key(native_edit))
        with_module = {**inputs, 'headers_sha256': {**inputs['headers_sha256'], 'native/buffer/local.h': 'one'}}
        module_edit = {**with_module, 'headers_sha256': {**inputs['headers_sha256'], 'native/buffer/local.h': 'two'}}
        self.assertEqual(build_hybrid.configuration_key(with_module, True), build_hybrid.configuration_key(module_edit, True))
        self.assertNotEqual(build_hybrid.configuration_key(with_module), build_hybrid.configuration_key(module_edit))

        for changed in [{**inputs, 'headers_sha256': {'abi.h': 'two'}},
                        {**inputs, 'modules': ['websocket']},
                        {**inputs, 'patches': [{'upstream_paths': ['config.m4'], 'sha256': 'two'}]}]:
            self.assertNotEqual(build_hybrid.configuration_key(inputs, True), build_hybrid.configuration_key(changed, True))

    def test_updates_preserve_unchanged_mtimes_and_restore_removed_patch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reference, source = root / 'reference', root / 'source'
            reference.mkdir()
            source.mkdir()
            subprocess.run(['git', 'init', '-q', str(reference)], check=True)
            (reference / 'value.cc').write_text('old\n')
            subprocess.run(['git', '-C', str(reference), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(reference), '-c', 'user.name=Test', '-c', 'user.email=test@example.invalid',
                            'commit', '-qm', 'fixture'], check=True)
            commit = subprocess.check_output(['git', '-C', str(reference), 'rev-parse', 'HEAD'], text=True).strip()
            (source / 'value.cc').write_text('old\n')
            patch_file = root / 'change.patch'
            patch_file.write_text('diff --git a/value.cc b/value.cc\n--- a/value.cc\n+++ b/value.cc\n@@ -1 +1 @@\n-old\n+new\n')
            patches = [{'path': str(patch_file), 'upstream_paths': ['value.cc']}]
            with patch.object(build_hybrid, 'COMMIT', commit):
                build_hybrid.refresh_dev_sources(reference, source, [], patches)
                self.assertEqual((source / 'value.cc').read_text(), 'new\n')
                modified = (source / 'value.cc').stat().st_mtime_ns
                build_hybrid.refresh_dev_sources(reference, source, patches, patches)
                self.assertEqual((source / 'value.cc').stat().st_mtime_ns, modified)
                build_hybrid.refresh_dev_sources(reference, source, patches, [])
                self.assertEqual((source / 'value.cc').read_text(), 'old\n')


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
    def test_new_real_caller_controls_use_selected_archive_without_runtime_claims(self):
        args = SimpleNamespace(lane='integrate', module=['reactor', 'phpcoroutines'],
                               all_modules=False, php='php', php_extension=[], jobs=2,
                               test=None, case=None, benchmark=False, compare=False,
                               stdext=False, fix_halfclose=False)
        commands = dict(dev.plan(args, ROOT / 'target/reports/real-callers'))
        queues = commands['reactor-callback-queues']
        self.assertIn('--sanitize', queues)
        catalogue = commands['phpcoroutines-method-catalogue']
        sanitized = commands['phpcoroutines-method-catalogue-native-sanitized']
        self.assertNotIn('--sanitize', catalogue)
        self.assertIn('--sanitize', sanitized)
        archives = {command[command.index('--archive') + 1]
                    for command in [queues, catalogue, sanitized]}
        self.assertEqual(len(archives), 1)
        for command in [queues, catalogue, sanitized]:
            self.assertNotIn('--engine', command)

    def test_long_all_module_aliases_publish_and_remain_distinct(self):
        from itertools import product
        from tools.build_hybrid import build_alias, MODULES
        variants = set()
        with tempfile.TemporaryDirectory() as directory:
            for repair, stdext in product([False, True], repeat=2):
                args = SimpleNamespace(lane='integrate', all_modules=True, module=[],
                                       fix_halfclose=repair, stdext=stdext)
                name = dev.build_variant(args)
                self.assertLessEqual(len(os.fsencode(name)), 255)
                (Path(directory)/name).mkdir()
                variants.add(name)
                self.assertEqual(name, build_alias('rust', reversed(sorted(MODULES)),
                    ('-halfclose' if repair else '') + ('-stdext' if stdext else '')))
            self.assertEqual(len(variants), 4)
        self.assertEqual(build_alias('rust', ['reactor'], '-halfclose-stdext'),
                         'rust-reactor-halfclose-stdext')

    def test_reactor_always_runs_owned_storage_gates(self):
        from itertools import product
        variants = set()
        for repair, stdext in product([False, True], repeat=2):
            args = SimpleNamespace(lane='integrate', module=['reactor'], all_modules=False,
                                   php='php', php_extension=[], jobs=2, test=None, case=None,
                                   benchmark=False, compare=False, stdext=stdext, fix_halfclose=repair)
            commands = dict(dev.plan(args, ROOT / 'target/reports/owned-config'))
            self.assertNotIn('-owned', ' '.join(map(str, commands['build'])))
            for phase, flags in [('reactor-production-owned-iov', ['--blocking', '--require-owned-iov']),
                                 ('reactor-production-owned-defer', ['--builtins', '--require-owned-defer', '--require-batch'])]:
                for flag in flags:
                    self.assertIn(flag, commands[phase])
            self.assertNotIn('reactor-production-owned-sockets', commands)
            self.assertEqual('--stdext' in commands['build'], stdext)
            self.assertEqual('--fix-halfclose' in commands['build'], repair)
            variants.add(dev.build_variant(args))
        self.assertEqual(len(variants), 4)

    def test_linux_consumer_hooks_load_curl_without_duplicating_explicit_dependency(self):
        for explicit in [[], ['curl']]:
            args = SimpleNamespace(lane='integrate', module=['phpcollections', 'base64', 'phphelpers'], all_modules=False,
                                   php='php', php_extension=explicit, jobs=2, test=None, case=None,
                                   benchmark=False, compare=False, fix_halfclose=False, stdext=True)
            with patch.object(dev.sys, 'platform', 'linux'):
                commands = dict(dev.plan(args, ROOT / 'target/reports/collections'))
            for phase in ['phpcollections-runtime',
                          'base64-consumers-upstream', 'base64-consumers-hybrid', 'phphelpers-entrypoint']:
                loaded = [commands[phase][i+1] for i, argument in enumerate(commands[phase])
                          if argument == '--php-extension']
                self.assertEqual(loaded.count('curl'), 1)

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
        self.assertNotIn('--match-child-php', matched['build'])
        self.assertNotIn('--match-child-php', matched['coroutine-native-originals'])

    def test_all_registered_modules_share_one_plan_and_close_gate(self):
        args = SimpleNamespace(lane='integrate', module=None, all_modules=True, php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False, fix_halfclose=False)
        with patch.object(dev.subprocess, 'check_output', return_value='/openssl'):
            commands = dict(dev.plan(args, ROOT / 'target/reports/all-registered'))
            # phphelpers' runtime fixtures call stdext helpers, so only stdext plans run them.
            stdext = dict(dev.plan(SimpleNamespace(**vars(args), stdext=True), ROOT / 'target/reports/all-registered-stdext'))
        for name in ['phphelpers-facade-runtime', 'phphelpers-entrypoint', 'phphelpers-runtime']:
            self.assertNotIn(name, commands)
        self.assertEqual(set(commands['build'][i+1] for i,x in enumerate(commands['build']) if x=='--module'),set(dev.SELECTABLE))
        self.assertIn('--sanitize', commands['reactor-poll-snapshot'])
        self.assertIn('--report', commands['reactor-poll-snapshot'])
        self.assertIn('--archive', commands['reactor-flush'])
        self.assertIn('--sanitize', commands['reactor-flush'])
        self.assertIn('corebase-native',commands)
        self.assertIn('http-native', commands)
        self.assertIn('--socket-output', commands['reactor-production-socket-output'])
        self.assertIn('coroutine-compile',commands)
        channel_state = commands['coroutine-channel-state']
        self.assertIn('tests/modules/coroutine/channel_state_gate.py', channel_state)
        self.assertEqual(channel_state[channel_state.index('--archive') + 1],
                         commands['coroutine-scheduler'][commands['coroutine-scheduler'].index('--archive') + 1])
        channel_sdk = commands['coroutine-channel-state-sdk']
        self.assertIn('--sdk-export', channel_sdk)
        self.assertIn('--native-export', channel_sdk)
        self.assertNotIn('--sdk-snapshot', channel_sdk)
        self.assertFalse(any(name.startswith('coroutine-registry') for name in commands))
        for name in ['hooks-ordered','hooks-async-policy','hooks-packet-policy','hooks-lifecycle-policy','hooks-socket-policy','hooks-compile']:
            self.assertIn(name,commands)
        self.assertIn('--config',commands['hooks-compile'])
        for name in ['reference-identity','base64-consumers-upstream','base64-consumers-hybrid',
                     'crc32-consumers-upstream','crc32-consumers-hybrid',
                     'coroutine-native-originals','corebase-mkdir','corebase-startup']:
            self.assertIn(name,commands)
        self.assertIn('--sanitize',commands['coroutine-native-originals'])
        self.assertNotIn('--inventory-only',commands['coroutine-native-originals'])
        # Upstream PHPTs run in the merge tier's phpt-* jobs, not as per-module phases (#83).
        self.assertNotIn('phpt', commands)
        self.assertEqual([name for name in commands if name.endswith(('-full-originals', '-originals-manifest'))], [])
        self.assertEqual(commands['framing-versioned-expectation'].count('--case'), 1)
        self.assertIn('tests/swoole_server/wrong_eof_setting.phpt', commands['framing-versioned-expectation'])
        self.assertIn('websocket-close',commands)
        self.assertIn('--export', commands['reference-build'])
        self.assertIn('--reference', commands['abi'])
        self.assertNotIn('--startup-only', commands['abi'])
        self.assertIn('--sanitize', commands['buffer-posix-iovec'])
        self.assertIn('--archive', commands['http-multipart-headers'])
        self.assertIn('--sanitize', commands['ports-flags'])
        self.assertIn('--sanitize', commands['ports-packet-settings'])
        self.assertIn('--sanitize', commands['ports-options'])
        self.assertIn('--core', commands['ports-options'])
        self.assertIn('--sanitize', commands['ports-tls-settings'])
        self.assertIn('--core', commands['ports-tls-settings'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[ownership]', commands['string-ownership'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[allocation]', commands['string-allocation'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[format]', commands['string-format'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[random]', commands['string-random'])
        self.assertIn('--source', commands['http-request-lifecycle'])
        self.assertIn('--archive', commands['http-request-lifecycle'])
        self.assertIn('--archive', commands['memory-fixed-native-construction'])
        self.assertIn('--archive', commands['memory-mapping-operation'])
        self.assertNotIn('--sanitize', commands['memory-mapping-operation'])
        self.assertIn('--archive', commands['reactor-backend-storage'])
        self.assertIn('--report', commands['reactor-backend-storage'])
        self.assertIn('--archive', commands['phpcoroutines-policy'])
        self.assertIn('--archive', commands['utilities-logger-owned-state'])
        self.assertIn('--sdk-ref', commands['utilities-logger-owned-state'])
        for family in ['logger', 'hash', 'inline', 'global-logger', 'redirect']:
            command = commands['utilities-' + family + '-originals']
            self.assertIn('--' + family + '-originals', command)
            self.assertTrue(any('selected-export/swoole.so' in arg for arg in command))
            self.assertNotEqual(command[command.index('--source-ref') + 1], 'HEAD')
        self.assertLess(list(commands).index('phphelpers-sdk'), list(commands).index('phpcoroutines-sdk'))
        self.assertIn('--core', commands['ports-flags'])
        self.assertLess(list(stdext).index('phphelpers-sdk'), list(stdext).index('phphelpers-facade-runtime'))
        self.assertNotIn('--prepare-only', stdext['phphelpers-facade-runtime'])
        self.assertEqual(stdext['phphelpers-facade-runtime'].count('--engine'), 2)
        self.assertIn('--source-ref', stdext['phphelpers-facade-runtime'])
        for family in ['logger', 'hash']:
            command = commands['utilities-' + family + '-originals']
            self.assertIn('--' + family + '-originals', command)
            self.assertEqual(command.count('--engine'), 2)
            self.assertIn('--source-ref', command)
        self.assertIn('--require-same-api', commands['api-surface'])
        self.assertEqual(commands['api-surface'].count('--engine'), 2)
        self.assertLess(list(commands).index('build'), list(commands).index('api-surface'))
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[originals]', commands['string-originals'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[split]', commands['string-split'])
        self.assertIn('memory-table-v2-compatibility', commands)
        self.assertIn('--core', commands['wrappers-listener'])
        self.assertIn('--sanitize', commands['wrappers-listener'])
        self.assertIn('column-bytes', commands['phpcollections-column-bytes'])
        self.assertEqual(commands['phpcollections-column-bytes'].count('--engine'), 2)
        self.assertIn('--core', commands['ports-defaults'])
        self.assertIn('--sanitize', commands['ports-defaults'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[integer]', commands['string-integer'])
        self.assertIn('--sanitize', commands['string-integer'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[copy]', commands['string-copy'])
        self.assertIn('--sanitize', commands['string-copy'])
        self.assertIn('--archive', commands['string-copy'])
        self.assertIn('tests/modules/buffer/string/test_gate.py::test_profile[transitions]', commands['string-transitions'])
        self.assertIn('--archive', commands['string-transitions'])
        self.assertIn('--source-ref', commands['utilities-public-storage'])
        self.assertIn('--build-manifest', commands['utilities-public-storage'])
        self.assertIn('--archive', commands['utilities-public-storage'])
        self.assertEqual(commands['utilities-public-storage'].count('--engine'), 2)
        self.assertIn('--threads', commands['utilities-put-threads'])
        self.assertIn('--sanitize', commands['utilities-put-threads'])
        self.assertIn('utilities-php-logger',commands)
        self.assertIn('--build-manifest', commands['utilities-php-logger'])
        self.assertIn('--archive', commands['utilities-php-logger'])
        self.assertIn('--source-ref', commands['utilities-php-logger'])
        self.assertIn('--archive', commands['httpbindings-server'])
        self.assertNotIn('httpbindings-php', commands)
        self.assertIn('--source', commands['memory-source-compatibility'])
        self.assertIn('--archive', commands['memory-source-compatibility'])
        self.assertIn('--build-manifest', commands['phpcollections-runtime'])
        self.assertIn('--archive', commands['phpcollections-runtime'])
        for phase in ['ports-count', 'ports-settings']:
            self.assertIn('--core', commands[phase])
            self.assertIn('--archive', commands[phase])
            self.assertIn('--sanitize', commands[phase])
            self.assertTrue(commands[phase][commands[phase].index('--core') + 1].endswith('/upstream/src'))
        self.assertIn('--sdk-export', commands['utilities-lowercase-sdk'])
        lowercase = commands['utilities-lowercase-runtime']
        for flag in ['--source', '--build-manifest', '--archive', '--source-ref']:
            self.assertIn(flag, lowercase)
        self.assertEqual(lowercase.count('--engine'), 2)
        self.assertNotIn('--case', lowercase)
        self.assertIn('--archive', commands['ports-listen'])
        self.assertIn('--archive', commands['ports-socket'])
        self.assertIn('--core', commands['ports-socket'])
        self.assertNotIn('ports-php', commands)
        self.assertNotIn('ports-callback-php', commands)
        for name in ['ports-configuration']:
            self.assertIn('--build-manifest', commands[name])
            self.assertIn('--archive', commands[name])
            self.assertIn('--source-ref', commands[name])
            self.assertNotIn('--case', commands[name])
        self.assertIn('--export', commands['build'])
        self.assertIn('--sdk-export', commands['phphelpers-sdk'])
        for provider in ['sqlite', 'odbc']:
            phase = 'phphelpers-async-' + provider + '-sdk'
            self.assertEqual(commands[phase][commands[phase].index('--provider') + 1], provider)
            self.assertIn('--sdk-report', commands[phase])
            self.assertLess(list(commands).index('phphelpers-sdk'), list(commands).index(phase))
            self.assertNotIn('--engine', commands[phase])
        self.assertIn('--sdk-report', stdext['phphelpers-entrypoint'])
        self.assertIn('--build-manifest', stdext['phphelpers-entrypoint'])
        self.assertIn('--archive', stdext['phphelpers-entrypoint'])
        self.assertEqual(stdext['phphelpers-entrypoint'].count('--engine'), 2)
        self.assertLess(list(commands).index('build'), list(commands).index('phphelpers-sdk'))
        self.assertLess(list(stdext).index('phphelpers-sdk'), list(stdext).index('phphelpers-entrypoint'))
        self.assertIn('--build-manifest', stdext['phphelpers-runtime'])
        self.assertIn('--archive', commands['phphelpers-policy'])
        asynchronous = commands['phphelpers-async']
        self.assertIn('tests/modules/phphelpers/async_gate.py', ' '.join(asynchronous))
        for flag in ['--source', '--archive', '--output']:
            self.assertIn(flag, asynchronous)
        self.assertEqual(asynchronous[asynchronous.index('--archive') + 1],
                         commands['phphelpers-policy'][commands['phphelpers-policy'].index('--archive') + 1])
        self.assertLess(list(commands).index('build'), list(commands).index('phphelpers-async'))
        self.assertNotIn('--sanitize', asynchronous)  # Intrinsic separate Native sanitizer controls.
        self.assertIn('--archive', commands['ports-policy'])
        self.assertIn('--archive', commands['http2-native'])
        self.assertIn('--sanitize', commands['http2-native'])
        for family in ['settings', 'control', 'header', 'queue', 'window', 'response-headers', 'publication', 'write', 'send', 'server-header', 'request-headers', 'response-builders', 'response-lifecycle', 'connection']:
            self.assertIn('--core-report', commands['http2-' + family])
            self.assertLess(list(commands).index('http2-native'), list(commands).index('http2-' + family))
        for name in ['clients-lifecycle', 'clients-write', 'clients-read', 'clients-connect', 'clients-persistent', 'clients-resolver-config', 'clients-transport', 'clients-pool', 'clients-resolver', 'clients-http-connection', 'clients-http-response', 'clients-http-format', 'clients-coro', 'clients-async', 'clients-object', 'clients-ip', 'clients-ip-text', 'clients-coro-io']:
            self.assertIn('--archive', commands[name])
            self.assertIn('--sanitize', commands[name])
        for family in ['emission', 'file']:
            self.assertIn('--archive', commands['httpbindings-' + family])
            self.assertNotIn('--without-compression', commands['httpbindings-' + family])
            self.assertIn('--without-compression', commands['httpbindings-' + family + '-without-compression'])
        self.assertIn('--without-zlib', commands['httpbindings-emission-without-zlib'])
        self.assertNotIn('--without-compression', commands['httpbindings-emission-without-zlib'])
        self.assertIn('--archive', commands['httpbindings-settings'])
        self.assertIn('--without-compression', commands['httpbindings-settings-without-compression'])
        self.assertNotIn('--faults', commands['clients-pool'])
        for family in ['http', 'dns', 'lifecycle', 'pool', 'resolver', 'pool_fork', 'http_connection', 'http_format', 'coro', 'async']:
            command = commands['clients-php-' + family]
            self.assertIn('--build-manifest', command)
            self.assertEqual(command.count('--engine'), 2)
            self.assertEqual('--source-ref' in command, family != 'lifecycle')
        self.assertIn('--archive',commands['clients-policy'])
        self.assertIn('--sanitize',commands['clients-policy'])
        self.assertIn('--archive',commands['clients-http'])
        self.assertIn('--sanitize',commands['clients-http'])
        self.assertIn('--archive',commands['clients-codec'])
        self.assertIn('--sanitize',commands['clients-codec'])
        self.assertIn('--archive', commands['corebase-lifecycle'])
        self.assertIn('--config', commands['corebase-lifecycle'])
        self.assertNotIn('--prepared', commands['corebase-lifecycle'])
        self.assertIn('--archive',commands['tls-native'])
        self.assertIn('--native-report',commands['tls-native'])
        self.assertIn('--allocation-native-report', commands['tls-native'])
        self.assertLess(list(commands).index('corebase-lifecycle'), list(commands).index('tls-native'))
        self.assertLess(list(commands).index('coroutine-native-originals'), list(commands).index('tls-native'))
        self.assertIn('--archive',commands['http-locales'])
        self.assertIn('--archive',commands['http-rewrites'])
        self.assertIn('--archive',commands['http-dispatch'])
        self.assertIn('--archive',commands['http-multipart-emit'])
        args.native_build_root = Path('/private-ram/hybrid')
        args.ccache = Path('/explicit/compiler-cache')
        with patch.object(dev.subprocess, 'check_output', return_value='/openssl'):
            cached_commands = dict(dev.plan(args, ROOT / 'target/reports/cached-plan'))
        self.assertEqual(cached_commands['build'][-2:], ['--ccache', '/explicit/compiler-cache'])
        command = cached_commands['build']
        self.assertEqual(command[command.index('--native-build-root') + 1], '/private-ram/hybrid')
        self.assertNotIn('--dev', command)
        self.assertIn('timer-bailout',commands)
        self.assertFalse(any(name.startswith('halfclose-') for name in commands))

    def test_stdext_changes_actual_configure_and_immutable_identity(self):
        args = SimpleNamespace(jobs=2, php_config='php-config', openssl_dir='/fixture/openssl', stdext=False)
        with patch.object(build_hybrid.subprocess, 'check_output', return_value='-I/fixture/pcre'):
            disabled, _ = build_hybrid.extension_configuration(args)
            args.stdext = True
            enabled, _ = build_hybrid.extension_configuration(args)
        self.assertNotIn('--enable-swoole-stdext', disabled)
        self.assertIn('--enable-swoole-stdext', enabled)
        self.assertNotEqual(build_hybrid.configuration_key({'effective_configure': disabled}),
                            build_hybrid.configuration_key({'effective_configure': enabled}))

    def test_php_sockets_configuration_is_explicit_and_not_inherited(self):
        args = SimpleNamespace(jobs=2, php_config='php-config', openssl_dir='/fixture/openssl')
        with patch.dict(os.environ, {'PHP_SOCKETS': 'yes'}), patch.object(
                build_hybrid.subprocess, 'check_output', return_value='-I/fixture/pcre'):
            disabled, _ = build_hybrid.extension_configuration(args)
            args.php_sockets = True
            enabled, _ = build_hybrid.extension_configuration(args)
        self.assertIn('--disable-sockets', disabled)
        self.assertNotIn('--enable-sockets', disabled)
        self.assertIn('--enable-sockets', enabled)
        self.assertNotEqual(build_hybrid.configuration_key({'effective_configure': disabled}),
                            build_hybrid.configuration_key({'effective_configure': enabled}))

    def test_php_sockets_selects_both_profiles_without_enabling_storage_in_native(self):
        from itertools import product
        for sockets, stdext in product([False, True], repeat=2):
            args = SimpleNamespace(lane='integrate', all_modules=True, module=None, php='php',
                                   php_extension=[], jobs=2, test=None, case=None, benchmark=False,
                                   compare=False, fix_halfclose=False, php_sockets=sockets,
                                   stdext=stdext)
            commands = dict(dev.plan(args, ROOT / 'target/reports/php-sockets'))
            native = commands['stdext-reference-build' if stdext else 'reference-build']
            for command in [commands['build'], native]:
                self.assertEqual('--php-sockets' in command, sockets)
            self.assertNotIn('--module', native)
            suffix = ('-php-sockets' if sockets else '') + ('-stdext' if stdext else '')
            self.assertIn('upstream=' + str(ROOT / ('target/hybrid/upstream' + suffix) / 'modules/swoole.so'),
                          commands['api-surface'])
            self.assertTrue(dev.build_variant(args).endswith(suffix))
            self.assertEqual('php-sockets-sendfile-original' in commands, sockets)
            if sockets:
                original = commands['php-sockets-sendfile-original']
                self.assertIn('tests/swoole_server/sendfile_02.phpt', original)
                self.assertIn('--require-all', original)
                self.assertIn('--isolate-cases', original)

    def test_php_sockets_reuse_rejects_mismatched_native_or_configured_profile(self):
        import hashlib
        profiles = [(False, False, False, False, True), (True, True, True, True, True),
                    (True, True, False, True, False), (True, False, True, True, False),
                    (True, True, True, False, False), (False, True, True, True, False)]
        for requested, selected, native, configured, accepted in profiles:
            with self.subTest(profile=(requested, selected, native, configured)), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                args = SimpleNamespace(lane='integrate', module=['http'], stdext=False,
                                       php_sockets=requested, fix_halfclose=False, artifacts_from=root/'exports')
                for label, variant in [('selected', dev.build_variant(args)),
                                       ('native', 'upstream' + ('-php-sockets' if requested else ''))]:
                    folder = args.artifacts_from/(label+'-export'); folder.mkdir(parents=True)
                    manifest = json.dumps(dict(build_mode='immutable',
                        configure=['--enable-sockets'] if configured else ['--disable-sockets'],
                        inputs=dict(modules=['http'], fix_halfclose=False,
                                    php_sockets_requested=selected if label=='selected' else native))).encode()
                    files = {}
                    for name, data in [('build-manifest.json', manifest), ('swoole.so', b'fixture only')]:
                        (folder/name).write_bytes(data); files[name] = {'sha256': hashlib.sha256(data).hexdigest()}
                        target = root/'target/hybrid'/variant/('modules/swoole.so' if name=='swoole.so' else name)
                        target.parent.mkdir(parents=True, exist_ok=True); target.write_bytes(data)
                    (folder/'export.json').write_text(json.dumps(dict(source_ref='fixture', files=files)))
                with patch.object(dev, 'ROOT', root), patch.object(dev.subprocess, 'check_output', return_value='fixture'):
                    if accepted:
                        self.assertEqual(set(dev.verify_artifact_report(args)), {'selected', 'native'})
                    else:
                        with self.assertRaisesRegex(ValueError, 'Artifact configuration differs'):
                            dev.verify_artifact_report(args)

    def test_stdext_uses_separate_real_native_and_rust_profiles(self):
        args = SimpleNamespace(lane='integrate', module=['phphelpers'], stdext=True, php='php',
                               php_extension=[], jobs=2, test=None, case=None, benchmark=False,
                               compare=False, fix_halfclose=False, native_build_root='/private-ram/hybrid')
        commands = dict(dev.plan(args, ROOT / 'target/reports/stdext-profiles'))
        self.assertEqual(dev.build_variant(args), 'rust-phphelpers-stdext')
        self.assertIn('--stdext', commands['build'])
        native = commands['stdext-reference-build']
        self.assertEqual(native[native.index('--backend') + 1], 'upstream')
        self.assertIn('--stdext', native)
        self.assertNotIn('--module', native)
        self.assertLess(list(commands).index('stdext-reference-build'), list(commands).index('build'))
        engines = [command for command in commands.values() if any(str(a).startswith('upstream=') for a in command)]
        self.assertTrue(engines)
        # Upstream engines come from the stdext reference build or its export.
        references = {'upstream=' + str(ROOT / 'target/hybrid/upstream-stdext/modules/swoole.so'),
                      'upstream=' + str(ROOT / 'target/reports/stdext-profiles/native-export/swoole.so')}
        for command in engines:
            self.assertTrue(references & set(map(str, command)), command)

    def test_all_module_build_receipt_uses_the_executed_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            variant = 'rust' + ''.join('-' + name for name in dev.SELECTABLE) + '-halfclose'
            receipt = root / 'target/hybrid' / variant / 'last-run.json'
            receipt.parent.mkdir(parents=True)
            manifest = receipt.parent / 'build-manifest.json'
            manifest.write_text('{"build_mode":"immutable"}')
            receipt.write_text(json.dumps(dict(identity='all-concrete-modules', build_manifest=str(manifest))))
            actual_run = subprocess.run
            def run(command, *args, **kwargs):
                return SimpleNamespace(returncode=0) if command == ['fixture-build'] else actual_run(command, *args, **kwargs)
            with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=[('build',['fixture-build'])]), \
                 patch.object(dev, 'require_build_space'), patch.object(dev, 'execute', side_effect=run), \
                 patch('sys.argv',['dev.py','integrate','--all-modules','--fix-halfclose']):
                self.assertEqual(dev.main(),0)
            reports = list(root.glob('target/reports/dev/*/loop.json'))
            self.assertEqual(len(reports),1)
            self.assertEqual(json.loads(reports[0].read_text())['phases'][0]['build']['identity'],'all-concrete-modules')

    def test_halfclose_repair_is_separate_from_rust_features_and_aliases(self):
        args = SimpleNamespace(lane='integrate', module=['error'], php='php', php_extension=[],
                               jobs=2, test=None, case=None, benchmark=False, compare=False,
                               fix_halfclose=True)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-repair'))
        self.assertIn('--fix-halfclose', commands['build'])
        self.assertEqual(commands['build'].count('--module'), 0)  # error is graduated
        self.assertIn('/rust-halfclose/', ' '.join(commands['error-native']))
        self.assertEqual(commands['error-php'].count('--engine'), 2)
        self.assertEqual(commands['halfclose-prequeue'].count('--engine'), 1)
        self.assertNotIn('--probe-coroutines', commands['halfclose-prequeue'])
        self.assertIn('--keep-alive', commands['halfclose-pressure'])
        stale = commands['halfclose-stale-read']
        self.assertEqual(stale[stale.index('--source') + 1], str(ROOT / 'target/hybrid/rust-halfclose/src'))

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
        self.assertIn('--allocation-failures', commands['protocols-native'])
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
        self.assertIn('/src', str(commands['halfclose-tls-native']))

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
        self.assertEqual(commands['halfclose-tls-native'][0], sys.executable)

    def test_integration_cannot_filter_or_use_development_build(self):
        from argparse import Namespace
        args = Namespace(lane='integrate', module=['timer'], php='php', php_extension=[],
                         jobs=4, test=None, case=None, benchmark=False, compare=False)
        commands = dict(dev.plan(args, ROOT / 'target/reports/test-plan'))
        self.assertNotIn('--dev', commands['build'])
        self.assertNotIn('phpt', commands)  # the merge tier's phpt-* jobs run every PHPT
        self.assertIn('--manifest', commands['timer-native'])
        self.assertNotIn('--dev', commands['timer-native'])


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
        self.assertEqual(self.changed('migration/patches/halfclose/manifest.json'), sorted(dev.MODULES))

    def test_selection_adds_the_modules_a_module_cannot_build_without(self):
        # build_hybrid.py refuses phpcollections without memory (TableV2 storage).
        selected = dev.selected_modules(SimpleNamespace(module=['phpcollections'], all_modules=False))
        self.assertIn('memory', selected)


class FocusedPhases(unittest.TestCase):
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
        for phase in ['ownership', 'build', 'reference-build', 'stdext-reference-build']:
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
