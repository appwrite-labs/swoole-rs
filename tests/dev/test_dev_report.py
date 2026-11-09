"""Frozen loop triage keeps unmatched, failed and unavailable evidence explicit."""
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import dev_report


class FrozenReports(unittest.TestCase):
    def fixture(self, root, results, code=1):
        root = Path(root)
        (root / 'cases.json').write_text(json.dumps({'results': results}))
        path = root / 'loop.json'
        path.write_text(json.dumps({'lane': 'integrate', 'status': 'failed', 'phases': [
            {'phase': 'cases', 'seconds': 20, 'returncode': code, 'log': str(root / 'missing.log'),
             'command': ['runner', '--output', str(root / 'cases.json')]},
            {'phase': 'missing', 'seconds': 1, 'returncode': 0,
             'command': ['runner', '--report', str(root / 'absent.json')]}]}))
        return path

    def test_engine_counts_boolean_failures_and_exact_case_pairing(self):
        rows = [
            {'case': 'same', 'engine': 'hybrid', 'status': 'failed'},
            {'case': 'same', 'engine': 'upstream', 'status': 'failed'},
            {'case': 'different', 'engine': 'hybrid', 'passed': False},
            {'case': 'different', 'engine': 'upstream', 'passed': True},
            {'case': 'sha', 'engine': 'hybrid', 'sha256': 'new', 'status': 'failed'},
            {'case': 'sha', 'engine': 'upstream', 'sha256': 'old', 'status': 'passed'},
            {'case': 'optional', 'engine': 'hybrid', 'status': 'unsupported'},
            {'case': 'warn', 'engine': 'hybrid', 'status': 'warned'},
            {'case': 'skip', 'engine': 'upstream', 'status': 'skipped'}]
        with tempfile.TemporaryDirectory() as root:
            summary = dev_report.inspect(self.fixture(root, rows), limit=1)
        outcome = summary['outcomes'][0]
        self.assertEqual(outcome['counts']['hybrid'], {'failed': 3, 'unsupported': 1, 'warned': 1})
        self.assertEqual(outcome['comparison_count'], 2)
        self.assertEqual(outcome['comparisons'][0]['relation'], 'different recorded statuses')
        self.assertEqual(summary['failed_phase_count'], 1)
        self.assertEqual(summary['failures'][0]['problem_count'], 7)
        self.assertEqual(summary['warnings'][0]['phase'], 'missing')
        self.assertEqual(summary['slowest'][0]['share_percent'], 95.24)

    def test_explicit_gate_failure_counts_even_when_process_exit_is_zero(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [], code=0)
            data = json.loads(path.read_text())
            data['phases'][0].update(failed=True, outcome_passed=False)
            path.write_text(json.dumps(data))
            summary = dev_report.inspect(path)
            self.assertEqual(summary['failed_phase_count'], 1)
            self.assertEqual(summary['failures'][0]['returncode'], 0)
            snapshot = dev_report.comparison_snapshot(path)
            self.assertEqual(snapshot['first_failed_phase']['returncode'], 0)
            data['status'] = 'passed'; path.write_text(json.dumps(data))
            for reader in [dev_report.inspect, dev_report.comparison_snapshot]:
                with self.assertRaisesRegex(ValueError, 'contains a failed phase'):
                    reader(path)

    def test_duplicates_and_backend_boundaries_are_not_attributed(self):
        rows = [{'case': 'duplicate', 'engine': engine, 'status': 'failed'}
                for engine in ['hybrid', 'hybrid', 'upstream']]
        rows += [{'case': 'backend', 'backend': engine, 'engine': engine, 'status': 'failed'}
                 for engine in ['hybrid', 'upstream']]
        counts, problems, comparisons = dev_report.outcomes({'results': rows})
        self.assertEqual(len(comparisons), 1)
        self.assertEqual(comparisons[0]['relation'], 'ambiguous repeated case')
        self.assertEqual(len(problems), 5)

    def test_oversized_and_malformed_artifacts_are_reported_even_on_success(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [], code=0)
            for raw in ['x' * 257, '{', '[]', '{"results":"bad"}']:
                (Path(root) / 'cases.json').write_text(raw)
                with patch.object(dev_report, 'MAX_JSON_BYTES', 256):
                    # The loop itself uses a separate larger bound; only its sidecar is oversized.
                    if len(raw) > 256:
                        with self.assertRaisesRegex(ValueError, 'inspection limit'):
                            dev_report.read_json(Path(root) / 'cases.json')
                        continue
                summary = dev_report.inspect(path, limit=1)
                self.assertEqual(summary['warnings'][0]['phase'], 'cases')

    def test_malformed_case_inventory_does_not_erase_independent_failed_gate(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [], code=0)
            artifact = Path(root) / 'cases.json'
            artifact.write_text(json.dumps({'results': 'malformed', 'passed': False,
                                           'error': 'retained independent gate failure'}))
            summary = dev_report.inspect(path)
            self.assertEqual(summary['outcomes'][0]['gate_status'], 'failed')
            self.assertEqual(summary['outcomes'][0]['gate_detail'], 'retained independent gate failure')
            self.assertFalse(summary['outcomes'][0]['counts'])
            details = [row['detail'] for row in summary['warnings']]
            self.assertTrue(any('Case inventory unavailable' in row for row in details))
            self.assertTrue(any('failed gate despite phase exit zero' in row for row in details))

    def test_large_log_is_read_from_tail_and_large_results_are_counted(self):
        with tempfile.TemporaryDirectory() as root:
            rows = [{'engine': 'hybrid', 'case': str(i), 'status': 'failed'} for i in range(20000)]
            path = self.fixture(root, rows)
            artifact = Path(root) / 'cases.json'
            artifact.write_text(json.dumps({'results': rows, 'checks': [
                {'status': 'unsupported'} for _ in range(20000)]}))
            (Path(root) / 'missing.log').write_text('begin\n' + 'x' * 2_000_000 + '\nend')
            summary = dev_report.inspect(path)
            self.assertEqual(summary['outcomes'][0]['nested_problem_count'], 20000)
            self.assertEqual(len(summary['outcomes'][0]['nested_problems']), 5)
            self.assertEqual(summary['outcomes'][0]['counts']['hybrid']['failed'], 20000)
            self.assertEqual(summary['failures'][0]['problem_count'], 20000)
            self.assertEqual(len(summary['failures'][0]['problems']), 8)
            self.assertTrue(summary['failures'][0]['excerpt'].endswith('end'))
            self.assertLessEqual(len(summary['failures'][0]['excerpt']), 4000)

    def test_build_identity_reads_only_recorded_immutable_manifest(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = self.fixture(root, [])
            manifest = root / 'build-manifest.json'
            manifest.write_text(json.dumps({'build_mode': 'immutable', 'extension': '/do/not/open.so',
                'extension_sha256': 'binary', 'source_diff_sha256': 'diff',
                'inputs': {'upstream_commit': 'pinned', 'modules': ['http'], 'fix_halfclose': True}}))
            data = json.loads(path.read_text())
            data['phases'][0]['build'] = {'build_manifest': str(manifest)}
            path.write_text(json.dumps(data))
            summary = dev_report.inspect(path)
            self.assertEqual(summary['identities'][0]['extension_sha256'], 'binary')
            self.assertEqual(summary['identities'][0]['upstream_commit'], 'pinned')
            manifest.write_text('{"build_mode":"incremental"}')
            summary = dev_report.inspect(path)
            self.assertFalse(summary['identities'])
            self.assertIn('not immutable', summary['warnings'][0]['detail'])

    def test_assertion_excerpt_is_not_hidden_by_official_runner_banner(self):
        raw = 'PHP runner banner\n' * 200 + 'Assert failed: original assertion\n' + 'x' * 1000
        detail = dev_report.problem_detail({'detail': raw})
        self.assertTrue(detail.startswith('Assert failed: original assertion'))
        self.assertEqual(len(detail), 600)
        self.assertEqual(dev_report.problem_detail({'detail': '001- expected\n001+ actual'}),
                         '001- expected\n001+ actual')

    def test_native_google_test_names_and_sanitizer_warnings_remain_distinct(self):
        counts, problems, comparisons = dev_report.outcomes({'results': [
            {'engine': 'rust', 'name': 'channel.close', 'sha256': 'fixture', 'status': 'failed'},
            {'engine': 'upstream', 'name': 'channel.close', 'sha256': 'fixture', 'status': 'passed'},
            {'engine': 'rust', 'name': 'context.bailout', 'sha256': 'fixture', 'status': 'warned',
             'sanitizer_warnings': ['actual no-return warning']}]})
        self.assertEqual(problems[0]['case'], 'channel.close')
        self.assertIn('actual no-return warning', problems[1]['detail'])
        self.assertEqual(comparisons[0]['rust'], ['failed'])
        self.assertEqual(comparisons[0]['upstream'], ['passed'])
        self.assertEqual(comparisons[0]['case'], 'channel.close')

    def test_completed_execution_does_not_override_boolean_gate_outcome(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = self.fixture(root, [], code=0)
            for passed, expected in [(True, 'passed'), (False, 'failed')]:
                (root / 'cases.json').write_text(json.dumps({'passed': passed, 'status': 'complete'}))
                summary = dev_report.inspect(path)
                self.assertEqual(summary['outcomes'][0]['execution_status'], 'complete')
                self.assertEqual(summary['outcomes'][0]['gate_status'], expected)
                if not passed:
                    self.assertIn('failed gate despite phase exit zero', summary['warnings'][0]['detail'])

    def test_gate_failure_and_native_error_are_visible_without_changing_recorded_status(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = self.fixture(root, [], code=0)
            (root / 'cases.json').write_text(json.dumps({'passed': False, 'results': [
                {'engine': 'hybrid', 'case': 'queued', 'passed': False, 'returncode': -11,
                 'stderr': '', 'error': 'Missing shutdown result', 'runner_log': 'exact/runner.log'}]}))
            summary = dev_report.inspect(path)
            self.assertEqual(summary['outcomes'][0]['gate_status'], 'failed')
            self.assertIn('failed gate despite phase exit zero', summary['warnings'][0]['detail'])
            data = json.loads(path.read_text())
            data['phases'][0]['returncode'] = 1
            data['phases'][0]['log'] = str(root / 'unavailable.log')
            path.write_text(json.dumps(data))
            summary = dev_report.inspect(path)
            problem = summary['failures'][0]['problems'][0]
            self.assertEqual(problem['detail'], 'Missing shutdown result')
            self.assertEqual(problem['returncode'], -11)
            self.assertEqual(problem['runner_log'], 'exact/runner.log')

    def test_engine_identity_is_deduplicated_by_full_observation_not_name(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = self.fixture(root, [{'engine': 'hybrid', 'case': 'one', 'status': 'failed'}])
            data = json.loads(path.read_text())
            for index, checksum in enumerate(['old', 'old', 'new']):
                artifact = root / f'engine-{index}.json'
                artifact.write_text(json.dumps({'engines': [{'name': 'hybrid', 'sha256': checksum, 'output': 'transcript' * 10000}]}))
                data['phases'].append({'phase': f'identity-{index}', 'seconds': 0, 'returncode': 0,
                                       'command': ['run', '--report', str(artifact)]})
            path.write_text(json.dumps(data))
            summary = dev_report.inspect(path)
            identities = summary['engine_identities']
            self.assertEqual([r['identity']['sha256'] for r in identities], ['old', 'new'])
            self.assertEqual([r['phases'] for r in identities], [['identity-0', 'identity-1'], ['identity-2']])
            self.assertEqual(len(identities[0]['reports']), 2)
            self.assertNotIn('output', identities[0]['identity'])
            self.assertLess(len(json.dumps(identities)), 2000)
            self.assertNotEqual(identities[0]['observation_sha256'], identities[1]['observation_sha256'])


    def test_linux_remapping_and_local_sibling_precede_stale_absolute_path(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            report = root / 'target/reports/dev/run/loop.json'
            report.parent.mkdir(parents=True)
            self.assertEqual(dev_report.sidecar('/work/target/ext/build-manifest.json', report),
                             root / 'target/ext/build-manifest.json')
            stale = root / 'stale.json'
            stale.write_text('{}')
            local = report.parent / 'stale.json'
            local.write_text('{}')
            self.assertEqual(dev_report.sidecar(str(stale), report), local)


    def test_resource_block_and_unsupported_coverage_are_not_hidden_by_zero_exits(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [], code=0)
            data = json.loads(path.read_text())
            data.update(blocked_phase='build', infrastructure_error='4 GiB disk guard blocked build')
            path.write_text(json.dumps(data))
            artifact = Path(root) / 'cases.json'
            artifact.write_text(json.dumps({'coverage_status': 'unsupported', 'codec_states': 0}))
            summary = dev_report.inspect(path)
            self.assertEqual(summary['blocked_phase'], 'build')
            self.assertEqual(summary['failed_phase_count'], 0)
            self.assertEqual(summary['outcomes'][0]['coverage_status'], 'unsupported')
            self.assertIsNone(summary['outcomes'][0]['gate_status'])
            self.assertFalse(summary['outcomes'][0]['counts'])
            result = subprocess.run([sys.executable, str(dev_report.ROOT / 'tools/dev_report.py'), str(path)],
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('4 GiB disk guard blocked build', result.stdout)
            self.assertIn('coverage=unsupported', result.stdout)

    def test_zero_exit_failure_keeps_first_assertion_in_cli_output(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [{'engine': 'hybrid', 'case': 'original', 'status': 'failed',
                'detail': 'Assert failed: retained first outcome', 'runner_log': 'official-runner.log'}], code=0)
            artifact = Path(root) / 'cases.json'
            data = json.loads(artifact.read_text())
            data.update(passed=False, error='gate receipt records failure')
            artifact.write_text(json.dumps(data))
            summary = dev_report.inspect(path, limit=1)
            self.assertEqual(summary['outcomes'][0]['problem_count'], 1)
            self.assertEqual(summary['outcomes'][0]['gate_detail'], 'gate receipt records failure')
            result = subprocess.run([sys.executable, str(dev_report.ROOT / 'tools/dev_report.py'), str(path)],
                                    text=True, capture_output=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('Assert failed: retained first outcome', result.stdout)
            self.assertIn('Runner: official-runner.log', result.stdout)
            self.assertIn('Gate detail: gate receipt records failure', result.stdout)
            self.assertEqual(result.stdout.count('Assert failed: retained first outcome'), 1)

    def test_nested_non_green_receipts_keep_paths_engines_and_original_values(self):
        data = {'passed': True, 'results': [
            {'engine': 'hybrid', 'status': 'failed', 'compile': {'returncode': 3}}],
            'engines': {'rust': {'run': {'returncode': -11}, 'checks': [
                {'status': 'unsupported'}, {'status': 'skipped'}, {'status': 'warned'},
                {'passed': False, 'error': 'missing prerequisite'}, {'returncode': 0},
                {'status': 502}, {'status': {'return_type': 'bool'}},
                {'status': 'PASS', 'passed': True}]}}}
        found = dev_report.nested_problems(data)
        self.assertEqual(len(found), 6)
        self.assertEqual(found[0]['path'], '$.results[0].compile')
        self.assertEqual(found[0]['engine'], 'hybrid')
        self.assertEqual(found[1]['returncode'], -11)
        self.assertEqual(found[1]['engine'], 'rust')
        self.assertEqual([row['status'] for row in found[2:5]], ['unsupported', 'skipped', 'warned'])
        self.assertEqual(found[-1]['detail'], 'missing prerequisite')
        self.assertEqual(data['results'][0]['status'], 'failed')

    def test_nested_coverage_and_execution_do_not_disappear_under_passed_status(self):
        data = {'results': [{'engine': 'hybrid', 'case': 'declared-pass', 'status': 'passed',
                             'coverage_status': 'unsupported'}],
                'engines': {'upstream': {'checks': [{'coverage_status': 'unsupported', 'status': 'passed'},
                                                 {'execution_status': 'interrupted'},
                                                 {'coverage_status': 200}, {'execution_status': 'complete'}]}}}
        found = dev_report.nested_problems(data)
        self.assertEqual(len(found), 3)
        self.assertEqual(found[0]['status'], 'passed')
        self.assertEqual(found[0]['coverage_status'], 'unsupported')
        self.assertEqual(found[0]['engine'], 'hybrid')
        self.assertEqual(found[1]['engine'], 'upstream')
        self.assertEqual(found[2]['execution_status'], 'interrupted')
        self.assertEqual(dev_report.outcomes(data)[0]['hybrid']['passed'], 1)

    def test_native_wrapper_exit_receipts_keep_their_child_log_pointer(self):
        data = {'commands': [{'execution': 'exited', 'exit': 1, 'log': 'attempt/native-originals.log'},
                             {'execution': 'exited', 'exit': 0, 'log': 'attempt/success.log'}]}
        found = dev_report.nested_problems(data)
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]['returncode'], 1)
        self.assertEqual(found[0]['log'], 'attempt/native-originals.log')
        self.assertEqual(found[0]['path'], '$.commands[0]')

    def test_passed_case_does_not_hide_recorded_warning_or_contradictory_boolean(self):
        data = {'results': [
            {'engine': 'rust', 'case': 'warning', 'status': 'passed',
             'sanitizer_warnings': ['recorded sanitizer warning']},
            {'engine': 'rust', 'case': 'contradiction', 'status': 'passed', 'passed': False}]}
        counts, _, _ = dev_report.outcomes(data)
        self.assertEqual(counts['rust'], {'passed': 2})  # Preserve the original case statuses.
        nested = dev_report.nested_problems(data)
        self.assertEqual(len(nested), 2)
        self.assertIn('recorded sanitizer warning', nested[0]['detail'])
        self.assertFalse(nested[1]['passed'])
        self.assertEqual(nested[1]['status'], 'passed')

    def test_nested_failure_visible_on_zero_exit_and_detail_limit_keeps_total(self):
        with tempfile.TemporaryDirectory() as root:
            path = self.fixture(root, [], code=0)
            artifact = Path(root) / 'cases.json'
            artifact.write_text(json.dumps({'passed': True, 'checks': [
                {'status': 'blocked'}, {'status': 'warned'}, {'returncode': 4}]}))
            summary = dev_report.inspect(path, limit=1)
            outcome = summary['outcomes'][0]
            self.assertEqual(outcome['gate_status'], 'passed')
            self.assertEqual(outcome['nested_problem_count'], 3)
            self.assertEqual(len(outcome['nested_problems']), 1)
            self.assertEqual(outcome['nested_problems'][0]['status'], 'blocked')
            self.assertEqual(summary['failed_phase_count'], 0)
            data = json.loads(path.read_text())
            data['status'] = 'running'
            path.write_text(json.dumps(data))
            summary = dev_report.inspect(path)
            self.assertTrue(summary['incomplete'])
            self.assertIsNone(summary['source_commit'])
            self.assertEqual(len(summary['report_sha256']), 64)



class FrozenComparisons(unittest.TestCase):
    def snapshot(self, root, name, statuses, *, checksum='assertion', reference='control', php='8.5',
                 state='failed', phase='originals', extra=()):
        directory = Path(root) / name
        directory.mkdir()
        artifact = directory / 'originals.json'
        artifact.write_text(json.dumps({'upstream_commit': 'pinned', 'engines': [
            {'name': 'hybrid', 'sha256': name, 'php': php, 'zts': False},
            {'name': 'upstream', 'sha256': reference, 'php': php, 'zts': False}], 'results': [
                {'engine': engine, 'test': case, 'sha256': checksum, 'status': status}
                for (engine, case), status in statuses] + list(extra)}))
        path = directory / 'loop.json'
        path.write_text(json.dumps({'lane': 'integrate', 'status': state, 'phases': [
            {'phase': phase, 'seconds': 1, 'returncode': 1, 'command': ['run', '--output', str(artifact)]}]}))
        return path

    def test_exact_case_transitions_keep_skips_and_both_engines(self):
        before = [('hybrid', 'new', 'passed'), ('hybrid', 'fixed', 'failed'),
                  ('hybrid', 'same', 'failed'), ('upstream', 'same', 'failed'), ('hybrid', 'optional', 'failed')]
        after = [('hybrid', 'new', 'failed'), ('hybrid', 'fixed', 'passed'),
                 ('hybrid', 'same', 'failed'), ('upstream', 'same', 'failed'), ('hybrid', 'optional', 'skipped')]
        def rows(values):
            return [((engine, case), status) for engine, case, status in values]
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', rows(before))
            new = self.snapshot(root, 'after', rows(after))
            summary = dev_report.compare(old, new, limit=1)
            self.assertEqual(summary['counts'], {'new_failures': 1, 'resolved_failures': 1,
                'unchanged_failures': 2, 'other_status_changes': 1, 'incomparable': 0})
            self.assertEqual(summary['new_failures'][0]['case'], 'new')
            self.assertEqual(summary['resolved_failures'][0]['case'], 'fixed')
            self.assertEqual(summary['other_status_changes'][0]['after'], 'skipped')
            self.assertEqual(len(summary['unchanged_failures']), 1)
            self.assertEqual(summary['before']['engine_identities'][0]['sha256'], 'before')

    def test_removed_test_or_phase_does_not_resolve_failure(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'removed'), 'failed')])
            new = self.snapshot(root, 'after', [])
            summary = dev_report.compare(old, new)
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Changed case/assertion/backend/engine inventory', summary['incomparable'][0]['reasons'])
            other = self.snapshot(root, 'other', [], phase='different')
            summary = dev_report.compare(old, other)
            self.assertEqual(summary['counts']['incomparable'], 2)
            self.assertFalse(summary['resolved_failures'])

    def test_repeated_phases_are_retained_in_exit_observations(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            data = json.loads(old.read_text())
            data['phases'].append(dict(data['phases'][0]))
            old.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['phase_exits'][0]['before'], [1, 1])
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Missing or repeated phase; absence is not resolution',
                          summary['incomparable'][0]['reasons'])

    def test_control_runtime_and_assertion_identity_changes_are_incomparable(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('upstream', 'case'), 'failed')])
            for index, arguments in enumerate([{'reference': 'changed'}, {'php': '8.2'}, {'checksum': 'changed'}]):
                new = self.snapshot(root, str(index), [(('upstream', 'case'), 'passed')], **arguments)
                summary = dev_report.compare(old, new)
                self.assertEqual(summary['counts']['incomparable'], 1)
                self.assertFalse(summary['resolved_failures'])

    def test_candidate_only_inventory_still_requires_unchanged_control_observation(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')], reference='different')
            summary = dev_report.compare(old, new)
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Changed runtime or reference identity: upstream', summary['incomparable'][0]['reasons'])

    def test_nested_engine_observations_do_not_crash_or_hide_named_cases(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            for path in [old, new]:
                artifact = path.parent / 'originals.json'
                data = json.loads(artifact.read_text())
                data['results'][0]['engine'] = {'name': 'hybrid', 'sha256': path.parent.name}
                artifact.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['counts']['resolved_failures'], 1)
            counts, problems, comparisons = dev_report.outcomes(json.loads((old.parent / 'originals.json').read_text()))
            self.assertEqual(counts['hybrid']['failed'], 1)
            self.assertEqual(problems[0]['engine'], 'hybrid')

    def test_native_named_cases_use_recorded_library_and_binary_control_identities(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            for path in [old, new]:
                artifact = path.parent / 'originals.json'
                data = json.loads(artifact.read_text())
                for engine in data['engines']:
                    if engine['name'] == 'hybrid':
                        engine['name'] = 'rust'
                    engine['library_sha256'] = engine.pop('sha256')
                for row in data['results']:
                    row['engine'] = 'rust'
                    row['name'] = row.pop('test')
                artifact.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['counts']['resolved_failures'], 1)
            self.assertEqual(summary['resolved_failures'][0]['case'], 'case')
            self.assertEqual(summary['resolved_failures'][0]['engine'], 'rust')

    def test_repeated_case_preserves_first_failure_and_receipt_pointer(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')], extra=[
                {'engine': 'hybrid', 'test': 'case', 'sha256': 'assertion', 'status': 'passed'}])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            raw = (old.parent / 'originals.json').read_bytes()
            summary = dev_report.compare(old, new)
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Repeated case identity; first outcomes remain ambiguous', summary['incomparable'][0]['reasons'])
            receipt = summary['incomparable'][0]['before_receipts'][0]
            self.assertEqual(receipt['path'], str((old.parent / 'originals.json').resolve()))
            self.assertEqual(raw, (old.parent / 'originals.json').read_bytes())

    def test_missing_receipt_and_zero_exit_are_not_resolution(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            (new.parent / 'originals.json').unlink()
            data = json.loads(new.read_text())
            data['phases'][0]['returncode'] = 0
            new.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['phase_exits'][0], {'phase': 'originals', 'before': [1], 'after': [0]})
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Missing case receipt', summary['incomparable'][0]['reasons'])

    def test_comparison_cli_preserves_failed_exit_and_rejects_active(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            command = [sys.executable, str(dev_report.ROOT / 'tools/dev_report.py'), str(new),
                       '--compare', str(old), '--json']
            result = subprocess.run(command, text=True, capture_output=True)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertEqual(json.loads(result.stdout)['counts']['resolved_failures'], 1)
            data = json.loads(new.read_text())
            data['status'] = 'running'
            new.write_text(json.dumps(data))
            result = subprocess.run(command, text=True, capture_output=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('requires completed', result.stderr)

    def test_incomparable_runs_expose_raw_status_changes_without_recovery_credit(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'kept'), 'failed'),
                (('hybrid', 'removed'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'kept'), 'passed')], reference='changed')
            summary = dev_report.compare(old, new, limit=1)
            self.assertFalse(summary['resolved_failures'])
            self.assertEqual(summary['unqualified_status_change_count'], 1)
            observed = summary['incomparable'][0]['unqualified_status_changes'][0]
            self.assertEqual((observed['case'], observed['before'], observed['after']),
                             ('kept', 'failed', 'passed'))
            self.assertIn('Changed case/assertion/backend/engine inventory', summary['incomparable'][0]['reasons'])
            data = json.loads((new.parent / 'originals.json').read_text())
            data['results'].append(dict(data['results'][0]))
            (new.parent / 'originals.json').write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['unqualified_status_change_count'], 0)
            self.assertFalse(summary['resolved_failures'])

    def test_engine_maps_preserve_native_binary_identities_without_invented_cases(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            for path in [old, new]:
                artifact = path.parent / 'originals.json'
                data = json.loads(artifact.read_text())
                data['engines'] = {engine['name']: {
                    'binary_sha256': engine['sha256'], 'compile': {'returncode': 0},
                    'run': {'returncode': 0, 'stdout': 'recorded native output'}} for engine in data['engines']}
                artifact.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['counts']['resolved_failures'], 1)
            identity = summary['after']['engine_identities'][1]
            self.assertEqual(identity['name'], 'upstream')
            self.assertEqual(identity['binary_sha256'], 'control')
            inspected = dev_report.inspect(new)
            self.assertEqual(len(inspected['engine_identities']), 2)
            self.assertIn('No log path recorded', inspected['failures'][0]['excerpt'])
            self.assertIsNone(inspected['failures'][0]['log'])
            self.assertNotIn('run', inspected['engine_identities'][0]['identity'])
            artifact = new.parent / 'originals.json'
            data = json.loads(artifact.read_text())
            data['results'] = []
            artifact.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Missing individual case inventory', summary['incomparable'][0]['reasons'])

    def test_unknown_phase_exit_cannot_recover_a_case(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            data = json.loads(new.read_text())
            data['phases'][0]['returncode'] = None
            new.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertEqual(summary['phase_change_counts'], {'unknown_exits': 1})
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Unknown phase exit; no recovery credit', summary['incomparable'][0]['reasons'])

    def test_phase_exit_triage_is_separate_from_incomparable_cases_and_times(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')], checksum='changed')
            before = json.loads(old.read_text())
            after = json.loads(new.read_text())
            before['phases'][0]['command'][0] = 'recorded-old-python'
            after['phases'][0]['command'][0] = 'recorded-new-python'
            before['phases'] += [dict(phase='slow', returncode=0, seconds=20, command=[]),
                                 dict(phase='removed', returncode=1, seconds=2, command=[])]
            after['phases'][0]['returncode'] = 0
            after['phases'] += [dict(phase='slow', returncode=9, seconds=40, command=[]),
                                dict(phase='added', returncode=1, seconds=3, command=[])]
            old.write_text(json.dumps(before)); new.write_text(json.dumps(after))
            summary = dev_report.compare(old, new, limit=1)
            self.assertEqual(summary['phase_change_counts'], {
                'added': 1, 'recovered_exits': 1, 'removed': 1, 'new_failures': 1})
            self.assertFalse(summary['resolved_failures'])
            recovered = summary['phase_changes']['recovered_exits'][0]
            self.assertEqual(recovered['phase'], 'originals')
            self.assertEqual(recovered['before'][0]['runner'], 'recorded-old-python')
            self.assertEqual(recovered['after'][0]['runner'], 'recorded-new-python')
            self.assertEqual(summary['runner_change_count'], 1)
            self.assertEqual(summary['runner_changes'][0], {'phase': 'originals',
                'before': 'recorded-old-python', 'after': 'recorded-new-python', 'both_recorded': True})
            self.assertEqual(summary['after']['slowest'][0]['seconds'], 40)
            self.assertEqual(summary['before']['slowest'][0]['seconds'], 20)
            self.assertIsNone(summary['after']['source_commit'])

    def test_timing_deltas_preserve_missing_repeated_and_unknown_observations(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [])
            new = self.snapshot(root, 'after', [])
            before, after = json.loads(old.read_text()), json.loads(new.read_text())
            before['phases'] += [dict(phase='missing', returncode=1, seconds=3),
                                 dict(phase='repeated', returncode=0, seconds=4),
                                 dict(phase='repeated', returncode=0, seconds=5),
                                 dict(phase='unknown', returncode=0, seconds='not recorded')]
            after['phases'][0]['seconds'] = 8
            after['phases'] += [dict(phase='repeated', returncode=0, seconds=2),
                               dict(phase='unknown', returncode=0, seconds=10)]
            old.write_text(json.dumps(before)); new.write_text(json.dumps(after))
            report = dev_report.compare(old, new, limit=1)
            rows = {row['phase']: row for row in report['phase_timings']}
            self.assertEqual(rows['originals']['delta_seconds'], 7)
            self.assertEqual(rows['missing']['after_seconds'], [])
            self.assertEqual(rows['repeated']['before_seconds'], [4, 5])
            self.assertEqual(rows['unknown']['before_seconds'], ['not recorded'])
            self.assertFalse(any(row['phase'] == 'unknown' for row in report['before']['slowest']))
            for name in ['missing', 'repeated', 'unknown']:
                self.assertIsNone(rows[name]['delta_seconds'])
            self.assertIn('preclude benchmark', report['timing_scope'])
            self.assertEqual(report['before']['first_failed_phase']['phase'], 'originals')

    def test_explicit_partial_comparison_retains_missing_phases_without_case_recovery(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')], state='running')
            before = json.loads(old.read_text())
            before['phases'].append(dict(phase='not-yet-recorded', seconds=9, returncode=1))
            old.write_text(json.dumps(before))
            report = dev_report.compare(old, new, allow_partial=True)
            self.assertTrue(report['incomplete'])
            self.assertFalse(report['resolved_failures'])
            self.assertEqual(report['phase_changes']['pending_or_absent_after'][0]['phase'], 'not-yet-recorded')
            self.assertTrue(all('Incomplete run; no qualified case transition credit' in row['reasons']
                                for row in report['incomparable']))
            command = [sys.executable, str(dev_report.ROOT / 'tools/dev_report.py'), str(new),
                       '--compare', str(old), '--allow-partial', '--json']
            result = subprocess.run(command, text=True, capture_output=True)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertTrue(json.loads(result.stdout)['after']['incomplete'])

    def test_runner_observation_changes_do_not_infer_missing_or_repeated_identities(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [])
            new = self.snapshot(root, 'after', [])
            before, after = json.loads(old.read_text()), json.loads(new.read_text())
            before['phases'][0]['command'] = []
            before['phases'] += [dict(phase='repeat', seconds=1, returncode=0, command=['old']),
                                 dict(phase='repeat', seconds=2, returncode=0, command=['old'])]
            after['phases'].append(dict(phase='repeat', seconds=3, returncode=0, command=['new']))
            old.write_text(json.dumps(before)); new.write_text(json.dumps(after))
            report = dev_report.compare(old, new)
            self.assertEqual(report['runner_change_count'], 1)
            self.assertFalse(report['runner_changes'][0]['both_recorded'])
            self.assertIsNone(report['runner_changes'][0]['before'])

    def test_nested_failures_prevent_case_recovery_credit(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            new = self.snapshot(root, 'after', [(('hybrid', 'case'), 'passed')])
            artifact = new.parent / 'originals.json'
            data = json.loads(artifact.read_text())
            data['run'] = {'returncode': -11}
            artifact.write_text(json.dumps(data))
            summary = dev_report.compare(old, new)
            self.assertFalse(summary['resolved_failures'])
            self.assertIn('Nested non-green observations', summary['incomparable'][0]['reasons'][0])
            self.assertEqual(summary['phase_changes']['unchanged_failures'][0]['after'][0][
                'nested_problems'][0]['returncode'], -11)

    def test_running_and_contradictory_reports_are_refused(self):
        with tempfile.TemporaryDirectory() as root:
            old = self.snapshot(root, 'before', [(('hybrid', 'case'), 'failed')])
            for status in ['running', 'interrupted', 'passed']:
                new = self.snapshot(root, status, [], state=status)
                with self.assertRaises(ValueError):
                    dev_report.compare(old, new)


if __name__ == '__main__':
    unittest.main()
