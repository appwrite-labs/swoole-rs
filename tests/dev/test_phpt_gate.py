"""The PHPT gate shards every test once, classifies skips and fails only on upstream passes cargo loses."""
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

from corpus.upstream.gate import category, compare, flagged, mark_reruns, markdown, pending, recheck, rows, skip_reason
from corpus.upstream.inventory import shard


class PhptGate(unittest.TestCase):
    def test_named_exclusion_never_becomes_a_pass_or_waives_a_failure(self):
        from corpus.upstream.run import named_exclusion, verdict_failed
        case = {'path': 'tests/swoole_thread/server/exit.phpt'}
        metadata = {'zts': 1, 'capabilities': ['threads']}
        self.assertIsNotNone(named_exclusion(case, metadata))
        self.assertIsNone(named_exclusion(case, {'zts': 0, 'capabilities': ['threads']}))
        self.assertIsNone(named_exclusion(case, {'zts': 1, 'capabilities': []}))
        self.assertIsNone(named_exclusion({'path': 'tests/other.phpt'}, metadata))
        self.assertFalse(verdict_failed({'test': case['path'], 'status': 'excluded'}, True))
        self.assertTrue(verdict_failed({'test': case['path'], 'status': 'failed',
                                        'upstream_status': 'WARNED'}, True))
        self.assertTrue(verdict_failed({'test': 'tests/other.phpt', 'status': 'excluded'}, True))
        report = [{'test': case['path'], 'family': 'swoole_thread', 'engine': engine,
                   'status': 'excluded', 'reason': named_exclusion(case, metadata)}
                  for engine in ['upstream', 'cargo']]
        summary = markdown(report)
        self.assertIn('0 passed, 0 failed, 0 skipped, 1 excluded', summary)
        self.assertIn('named originals excluded (not passed)', summary)
        self.assertEqual(compare(report), ([], []))

    @mock.patch.dict('corpus.upstream.gate.PENDING', {'tests/a/failing.phpt': 1, 'tests/a/passing.phpt': 1})
    def test_pending_port_is_listed_never_passed_and_its_passes_are_flagged(self):
        from corpus.upstream.gate import PENDING
        failing, passing = sorted(PENDING)[:2]
        report = [{'test': test, 'family': 'f', 'engine': engine, 'status': status}
                  for test, cargo in [(failing, 'failed'), (passing, 'passed'), ('tests/other.phpt', 'failed')]
                  for engine, status in [('upstream', 'passed'), ('cargo', cargo)]]
        self.assertEqual(compare(report)[0], ['tests/other.phpt (cargo: failed)'])
        summary = markdown(report)
        self.assertIn(f'1 pending a port (not gated)</summary>\n\n- `{failing} (#{PENDING[failing]}): failed`', summary)
        self.assertIn(f'1 pending a port but passing here: drop them from PENDING_PORTS</summary>\n\n- `{passing}`', summary)

    def test_thread_context_racy_original_runs_without_thread_context(self):
        from corpus.upstream.run import racy, verdict_failed
        case = {'path': 'tests/swoole_coroutine/cancel/throw_exception.phpt'}
        for path in ['tests/swoole_coroutine/cancel/throw_exception.phpt', 'tests/swoole_coroutine/stats.phpt']:
            self.assertIsNotNone(racy({'path': path}, {'capabilities': ['thread_context']}))
            self.assertIsNone(racy({'path': path}, {'capabilities': []}))
        self.assertIsNotNone(racy({'path': 'tests/swoole_server/close_max_fd.phpt'}, {'capabilities': []}))
        self.assertIsNotNone(racy({'path': 'tests/swoole_runtime/file_lock/async_file.phpt'}, {'capabilities': []}))
        self.assertTrue(verdict_failed({'test': case['path'], 'status': 'failed'}, True))

    def test_shards_partition_every_path(self):
        paths = [f'tests/t/{n}.phpt' for n in range(25)]
        parts = [shard(paths, index, 4) for index in range(1, 5)]
        self.assertEqual(sorted(p for part in parts for p in part), sorted(paths))
        self.assertTrue(all(abs(len(part) - 25 / 4) < 1 for part in parts))

    def test_family_selection_comes_before_sharding(self):
        from corpus.upstream import inventory
        paths = [f'tests/{family}/{n}.phpt' for family in ['a', 'b', 'c'] for n in range(4)] + ['tests/a/fixture.inc']
        with tempfile.TemporaryDirectory() as temp, mock.patch.object(inventory, 'blobs', return_value=paths), \
             mock.patch.object(inventory, 'digest', return_value='0'):
            shards = []
            for index in (1, 2):
                output = Path(temp) / f'{index}.json'
                with mock.patch('sys.argv', ['inventory.py', '--module', 'a', '--module', 'c', '--shard', f'{index}/2',
                                             '--output', str(output)]):
                    self.assertEqual(inventory.main(), 0)
                manifest = json.loads(output.read_text())
                shards.append([test['path'] for test in manifest['tests']])
                self.assertEqual([row['path'] for row in manifest['support_files']], ['tests/a/fixture.inc'])
        # The selected families' eight tests split evenly; family b is left out.
        self.assertEqual([len(tests) for tests in shards], [4, 4])
        self.assertEqual(sorted(t for tests in shards for t in tests), sorted(p for p in paths if p.endswith('.phpt') and '/b/' not in p))

    def test_the_shard_matrix_grows_with_the_tests_selected(self):
        from corpus.upstream import inventory
        paths = [f'tests/{family}/{n}.phpt' for family, count in [('small', 3), ('server', 221), ('big', 2000)]
                 for n in range(count)]
        for modules, matrix in [(['none'], [1]), (['small'], [1]), (['server'], [1, 2]),
                                (['small', 'server'], [1, 2]), ([], [1, 2, 3, 4, 5, 6, 7, 8, 9])]:
            with mock.patch.object(inventory, 'blobs', return_value=paths), mock.patch('sys.stdout', new=io.StringIO()) as out, \
                 mock.patch('sys.argv', ['inventory.py', '--plan', *(f'--module={module}' for module in modules)]):
                self.assertEqual(inventory.main(), 0)
            self.assertEqual(json.loads(out.getvalue()), matrix, modules)

    def test_skips_are_classified_and_only_passed_counts(self):
        detail = 'SKIP x [tests/x.phpt] reason: \x1b[33mmysql server not available\x1b[0m\n'
        self.assertEqual(skip_reason(detail), 'mysql server not available')
        self.assertEqual(category('mysql server not available'), 'service')
        self.assertEqual(category('skip on ZTS'), 'zts')
        self.assertEqual(category('not support in nts'), 'zts')
        self.assertEqual(category('for select'), 'other')
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'shard.json'
            report.write_text(json.dumps({'results': [
                {'test': 'tests/a/1.phpt', 'module': 'a', 'status': 'passed', 'upstream_status': 'PASSED'},
                {'test': 'tests/a/2.phpt', 'module': 'a', 'status': 'skipped', 'upstream_status': 'SKIPPED', 'detail': detail},
                {'test': 'tests/b/1.phpt', 'module': 'b', 'status': 'failed', 'upstream_status': 'WARNED'},
                {'test': 'tests/b/2.phpt', 'module': 'b', 'status': 'failed', 'upstream_status': 'FAILED',
                 'detail': '001+ ** ERROR: process timed out **'}]}))
            results = list(rows([report]))
        self.assertEqual([r['status'] for r in results], ['passed', 'skipped', 'failed', 'failed'])
        self.assertEqual([r.get('timed_out') for r in results[2:]], [False, True])
        results = results[:3]
        self.assertIn('1 passed, 1 failed, 1 skipped', markdown(results))
        self.assertIn('| a | 1 | 0 | 1 |', markdown(results))

    def test_only_upstream_passes_gate_cargo(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        results = [row('kept', 'upstream', 'passed'), row('kept', 'cargo', 'passed'),
                   row('lost', 'upstream', 'passed'), row('lost', 'cargo', 'failed'),
                   row('absent', 'upstream', 'passed'),
                   row('both', 'upstream', 'failed'), row('both', 'cargo', 'failed'),
                   row('fixed', 'upstream', 'failed'), row('fixed', 'cargo', 'passed'),
                   row('skip', 'upstream', 'skipped'), row('skip', 'cargo', 'failed')]
        self.assertEqual(compare(results), (['absent (cargo: missing)', 'lost (cargo: failed)'], ['both']))
        self.assertEqual(compare([row('only', 'upstream', 'failed')]), ([], []))
        self.assertIn('2 passing on upstream but not here', markdown(results))

    def test_reruns_separate_regressions_from_flakes(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        results = [row(t, 'upstream', 'passed') for t in ['real', 'upstream-flake', 'cargo-blip', 'unrun', 'kept']]
        results += [row(t, 'cargo', 'failed') for t in ['real', 'upstream-flake', 'cargo-blip', 'unrun']]
        results += [row('kept', 'cargo', 'passed')]
        self.assertEqual(flagged(results), ['cargo-blip', 'real', 'unrun', 'upstream-flake'])
        rechecked = [row('real', 'upstream', 'passed')] * 3 + [row('real', 'cargo', 'passed'), row('real', 'cargo', 'failed')]
        rechecked += [row('upstream-flake', 'upstream', 'failed'), row('upstream-flake', 'cargo', 'failed'),
                      row('upstream-flake', 'cargo', 'passed')]
        rechecked += [row('cargo-blip', 'upstream', 'passed'), row('cargo-blip', 'cargo', 'passed')]
        # 'unrun' has no re-runs, so it stays a regression.
        self.assertEqual(recheck(flagged(results), rechecked),
                         (['real', 'unrun'], ['upstream-flake'], ['cargo-blip'], []))
        summary = markdown(results, rechecked)
        self.assertIn('2 regressions: pass on upstream, fail on a re-run here', summary)
        self.assertIn('1 flaky on upstream', summary)
        self.assertIn('1 passed on re-run on both engines', summary)
        # The badge counts a cargo pass on an ungated re-run; a regression's passing re-run doesn't count.
        mark_reruns(results, rechecked)
        self.assertEqual(sorted(r['test'] for r in results if r.get('rerun') == 'passed'), ['cargo-blip', 'upstream-flake'])

    def test_upstream_flakes_still_gate_cargo_that_always_fails(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        rechecked = [row('broken', 'upstream', 'passed'), row('broken', 'upstream', 'failed')]
        rechecked += [row('broken', 'cargo', 'failed')] * 3
        rechecked += [row('flaky', 'upstream', 'failed'), row('flaky', 'cargo', 'failed'), row('flaky', 'cargo', 'passed')]
        rechecked += [row('unrun', 'upstream', 'failed')]
        # Upstream failing a re-run no longer excuses cargo that never passes.
        self.assertEqual(recheck(['broken', 'flaky', 'unrun'], rechecked), (['broken', 'unrun'], ['flaky'], [], []))
        summary = markdown([row(t, 'upstream', 'passed') for t in ['broken', 'flaky']] +
                           [row(t, 'cargo', 'failed') for t in ['broken', 'flaky']], rechecked)
        self.assertIn('1 regressions', summary)
        self.assertIn('- `flaky`: cargo failed, passed; upstream failed', summary)

    def test_reruns_stop_once_settled_and_a_timeout_gets_one(self):
        def row(test, engine, status, timed_out=False):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other',
                    'timed_out': timed_out}
        tests = ['flaky-passed', 'flaky-failing', 'steady-failed', 'steady-passing', 'slow']
        rechecked = [row('flaky-passed', 'upstream', 'failed'), row('flaky-passed', 'cargo', 'passed'),
                     row('flaky-failing', 'upstream', 'failed'), row('flaky-failing', 'cargo', 'failed'),
                     row('steady-failed', 'upstream', 'passed'), row('steady-failed', 'cargo', 'failed'),
                     row('steady-passing', 'upstream', 'passed'), row('steady-passing', 'cargo', 'passed'),
                     row('slow', 'upstream', 'failed'), row('slow', 'cargo', 'failed', True)]
        results = [row('slow', 'cargo', 'failed', True)]
        # Upstream stops at its first failure; cargo until its verdict is settled.
        self.assertEqual(pending(tests, rechecked, 'upstream'), ['steady-failed', 'steady-passing'])
        self.assertEqual(pending(tests, rechecked, 'cargo', results), ['flaky-failing', 'steady-failed', 'steady-passing'])
        # Once upstream failed, a test racy on both engines gets nine cargo re-runs to pass one.
        self.assertEqual(pending(['flaky-failing'], rechecked + [row('flaky-failing', 'cargo', 'failed')] * 2,
                                 'cargo', results), ['flaky-failing'])
        self.assertEqual(pending(['flaky-failing'], rechecked + [row('flaky-failing', 'cargo', 'failed')] * 8,
                                 'cargo', results), [])
        # One failed re-run doesn't settle cargo on a clean upstream; a pass then does (#862),
        # and three failures spend the budget.
        self.assertEqual(pending(['steady-failed'], rechecked + [row('steady-failed', 'cargo', 'passed')], 'cargo'), [])
        self.assertEqual(pending(['steady-failed'], rechecked + [row('steady-failed', 'cargo', 'failed')] * 2, 'cargo'), [])
        # A first cargo failure that timed out gets one re-run; timing out again is a regression.
        self.assertEqual(recheck(['slow'], rechecked), (['slow'], [], [], []))
        self.assertEqual(pending(['slow'], [row('slow', 'upstream', 'failed')], 'cargo', results), ['slow'])

    def test_mixed_cargo_gets_more_upstream_reruns(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        tests = ['mixed', 'broken', 'clean']
        rechecked = [row(t, 'upstream', 'passed') for t in tests for _ in range(3)]
        rechecked += [row('mixed', 'cargo', 'passed'), row('mixed', 'cargo', 'failed')]
        rechecked += [row('broken', 'cargo', 'failed')] * 3 + [row('clean', 'cargo', 'passed')] * 3
        # Only cargo both passing and failing earns upstream re-runs past three.
        self.assertEqual(pending(tests, rechecked, 'upstream'), [])
        self.assertEqual(pending(tests, rechecked, 'upstream-extended'), ['mixed'])
        self.assertEqual(recheck(['mixed'], rechecked), (['mixed'], [], [], []))
        # An upstream failure among them makes it flaky on both; 24 clean passes keep the regression (#1285).
        racy = rechecked + [row('mixed', 'upstream', 'passed'), row('mixed', 'upstream', 'failed')]
        self.assertEqual(pending(['mixed'], racy, 'upstream-extended'), [])
        self.assertEqual(recheck(['mixed'], racy), ([], ['mixed'], [], []))
        solid = rechecked + [row('mixed', 'upstream', 'passed')] * 20
        self.assertEqual(pending(['mixed'], solid, 'upstream-extended'), ['mixed'])
        solid.append(row('mixed', 'upstream', 'passed'))
        self.assertEqual(pending(['mixed'], solid, 'upstream-extended'), [])
        self.assertEqual(recheck(['mixed'], solid), (['mixed'], [], [], []))

    def test_a_test_both_engines_almost_always_fail_needs_an_upstream_pass_to_gate(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        # Upstream passed the first run, then failed a re-run; cargo failed all nine (#998).
        rechecked = [row('rare', 'upstream', 'failed')] + [row('rare', 'cargo', 'failed')] * 9
        self.assertEqual(pending(['rare'], rechecked, 'upstream-extended'), ['rare'])
        self.assertEqual(recheck(['rare'], rechecked), (['rare'], [], [], []))
        # Upstream keeps re-running to nine in all, past a pass.
        allfail = rechecked + [row('rare', 'upstream', 'failed')] * 8
        self.assertEqual(pending(['rare'], allfail, 'upstream-extended'), [])
        self.assertEqual(recheck(['rare'], allfail), ([], ['rare'], [], []))
        passed = rechecked + [row('rare', 'upstream', 'failed'), row('rare', 'upstream', 'passed')]
        self.assertEqual(pending(['rare'], passed, 'upstream-extended'), ['rare'])
        self.assertEqual(recheck(['rare'], passed), (['rare'], [], [], []))
        # Two upstream passes in nine against none of cargo's is a race on both (#1948);
        # three is a third, and gates it.
        racy = passed + [row('rare', 'upstream', 'failed')] * 5 + [row('rare', 'upstream', 'passed')]
        self.assertEqual(pending(['rare'], racy, 'upstream-extended'), [])
        self.assertEqual(recheck(['rare'], racy), ([], ['rare'], [], []))
        solid = passed + [row('rare', 'upstream', 'failed')] * 4 + [row('rare', 'upstream', 'passed')] * 2
        self.assertEqual(pending(['rare'], solid, 'upstream-extended'), [])
        self.assertEqual(recheck(['rare'], solid), (['rare'], [], [], []))

    def test_the_runner_abort_is_inconclusive_on_either_engine(self):
        abort = 'PASS x [tests/x.phpt]\n...\ncorrupted double-linked list\nAborted (core dumped)\n'
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'recheck.json'
            report.write_text(json.dumps({'results': [
                {'test': 'tests/x.phpt', 'module': 'x', 'status': 'failed', 'upstream_status': 'PASSED', 'detail': abort},
                {'test': 'tests/y.phpt', 'module': 'y', 'status': 'failed', 'upstream_status': 'FAILED', 'detail': abort},
                {'test': 'tests/w.phpt', 'module': 'w', 'status': 'failed', 'upstream_status': 'WARNED', 'detail': abort},
                {'test': 'tests/z.phpt', 'module': 'z', 'status': 'failed', 'upstream_status': 'PASSED',
                 'detail': 'Segmentation fault (core dumped)'}]}))
            # Only a passing test followed by the #200 signature; never a failed test (WARNED failed
            # its first attempt, #1300) or other crashes.
            self.assertEqual([r['inconclusive'] for r in rows([report])], [True, False, False, False])

        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': 'failed' if status == 'aborted' else status,
                    'category': 'other', 'inconclusive': status == 'aborted'}
        rechecked = [row(t, 'upstream', 'passed') for t in ['replaced', 'regressed', 'never'] for _ in range(3)]
        rechecked += [row('replaced', 'cargo', 'aborted'), row('replaced', 'cargo', 'passed'),
                      row('replaced', 'cargo', 'passed'), row('replaced', 'cargo', 'passed')]
        rechecked += [row('regressed', 'cargo', 'aborted'), row('regressed', 'cargo', 'failed')]
        rechecked += [row('never', 'cargo', 'aborted')] * 6
        rechecked += [row('upstream-only', 'upstream', 'aborted')] * 6 + [row('upstream-only', 'cargo', 'passed')] * 3
        tests = ['never', 'regressed', 'replaced', 'upstream-only']
        self.assertEqual(recheck(tests, rechecked), (['regressed'], [], ['replaced'], ['never', 'upstream-only']))
        # An inconclusive re-run gets one more in its place, up to twice the budget in all.
        self.assertEqual(pending(['replaced'], rechecked[:3] + [row('replaced', 'cargo', 'aborted')], 'cargo'), ['replaced'])
        self.assertEqual(pending(['never'], rechecked, 'cargo'), [])
        self.assertEqual(pending(['x'], [row('x', 'upstream', 'aborted')] * 2 + [row('x', 'upstream', 'passed')], 'upstream'), ['x'])
        self.assertEqual(pending(['upstream-only'], rechecked, 'upstream'), [])
        summary = markdown([row(t, 'upstream', 'passed') for t in tests] + [row(t, 'cargo', 'failed') for t in tests], rechecked)
        self.assertIn('2 inconclusive: every re-run on an engine ended in the runner abort', summary)

    def test_debug_php_reports_are_listed_per_engine_and_gated_as_failures(self):
        leak = ("004+ [Mon Nov  9 03:12:44 2026]  Script:  '/t/tests/x.php'\n"
                '005+ Zend/zend_string.h(176) :  Freed 0x7f0 (32 bytes), script=/t/tests/x.php\n'
                '006+ === Total 1 memory leaks detected ===\n')
        assertion = "002+ php: Zend/zend_hash.c:1075: _zend_hash_index_add: Assertion `(ht)->nTableMask' failed.\n"
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'shard.json'
            report.write_text(json.dumps({'results': [
                {'test': 'tests/x.phpt', 'module': 'x', 'engine': 'upstream', 'status': 'passed', 'upstream_status': 'PASSED'},
                {'test': 'tests/x.phpt', 'module': 'x', 'engine': 'cargo', 'status': 'failed', 'upstream_status': 'FAILED', 'detail': leak},
                {'test': 'tests/y.phpt', 'module': 'y', 'engine': 'upstream', 'status': 'failed', 'upstream_status': 'FAILED', 'detail': leak},
                {'test': 'tests/y.phpt', 'module': 'y', 'engine': 'cargo', 'status': 'failed', 'upstream_status': 'FAILED', 'detail': assertion},
                {'test': 'tests/z.phpt', 'module': 'z', 'engine': 'cargo', 'status': 'failed', 'upstream_status': 'FAILED',
                 'detail': '001+ ** ERROR: process timed out **'}]}))
            results = list(rows([report]))
        self.assertEqual([r.get('report') for r in results], [None, 'leak', 'leak', 'assertion', None])
        self.assertEqual(compare(results)[0], ['tests/x.phpt (cargo: failed)'])
        summary = markdown(results)
        self.assertIn('3 leak or assertion reports', summary)
        self.assertIn('- `tests/y.phpt (upstream: leak)`', summary)
        self.assertNotIn('leak or assertion', markdown(results[:1]))

    def test_incomplete_shards_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'shard.json'
            report.write_text(json.dumps({'execution_status': 'interrupted', 'results': []}))
            with self.assertRaises(ValueError):
                list(rows([report]))

    def test_regressions_are_annotated_only_under_github_actions(self):
        import os
        import subprocess
        import sys
        root = Path(__file__).resolve().parents[2]
        results = [{'test': f'tests/t/{n}.phpt', 'module': 't', 'engine': engine, 'status': status}
                   for n in range(12) for engine, status in [('upstream', 'passed'), ('cargo', 'failed')]]
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'r.json'
            report.write_text(json.dumps({'results': results}))
            def gate(**env):
                environment = {key: value for key, value in os.environ.items() if key != 'GITHUB_ACTIONS'}
                return subprocess.run([sys.executable, root / 'corpus/upstream/gate.py', report],
                                      env={**environment, **env}, capture_output=True, text=True)
            self.assertNotIn('::error', gate().stderr)
            lines = gate(GITHUB_ACTIONS='true').stderr.splitlines()
        # Nine by name and one count: GitHub keeps about ten annotations of a level per step.
        self.assertEqual(lines[0], '::error title=PHPT regression::tests/t/0.phpt (cargo: failed)')
        self.assertEqual(len(lines), 10)
        self.assertEqual(lines[-1], '::error title=3 more errors::3 more past the first 9; the log lists them all')

    def test_annotation_properties_escape_their_separators(self):
        from tools.annotations import escape
        self.assertEqual(escape('a: b, 100%\nc', separators=True), 'a%3A b%2C 100%25%0Ac')
        self.assertEqual(escape('a: b, c'), 'a: b, c')

    def test_concurrent_annotations_keep_the_step_budget(self):
        # dev.py's phase threads annotate at once: nine by name, the rest counted.
        import threading
        from tools import annotations
        emitted = []
        with mock.patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'}), \
                mock.patch.object(annotations, '_shown', annotations.Counter()), \
                mock.patch.object(annotations, '_omitted', annotations.Counter()), \
                mock.patch.object(annotations, 'emit', lambda *args: emitted.append(args)), \
                mock.patch.object(annotations.atexit, 'register'):
            self.addCleanup(sys.setswitchinterval, sys.getswitchinterval())
            sys.setswitchinterval(1e-6)
            start = threading.Barrier(16)
            def worker(n):
                start.wait()
                for i in range(50):
                    annotations.annotate('error', f'{n}.{i} failed', 'exit 1')
            threads = [threading.Thread(target=worker, args=(n,)) for n in range(16)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            self.assertEqual(len(emitted), annotations.PER_STEP - 1)
            self.assertEqual(annotations._omitted['error'], 16 * 50 - (annotations.PER_STEP - 1))


if __name__ == '__main__':
    unittest.main()
