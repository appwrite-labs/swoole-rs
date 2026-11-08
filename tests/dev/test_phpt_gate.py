"""The PHPT gate shards every test once, classifies skips and fails only on upstream passes the hybrid loses."""
import json
from pathlib import Path
import tempfile
import unittest

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
                  for engine in ['upstream', 'hybrid']]
        summary = markdown(report)
        self.assertIn('0 passed, 0 failed, 0 skipped, 1 excluded', summary)
        self.assertIn('named originals excluded (not passed)', summary)
        self.assertEqual(compare(report), ([], []))

    def test_pending_port_is_listed_never_passed_and_its_passes_are_flagged(self):
        from corpus.upstream.gate import PENDING
        failing, passing = sorted(PENDING)[:2]
        report = [{'test': test, 'family': 'f', 'engine': engine, 'status': status}
                  for test, hybrid in [(failing, 'failed'), (passing, 'passed'), ('tests/other.phpt', 'failed')]
                  for engine, status in [('upstream', 'passed'), ('hybrid', hybrid)]]
        self.assertEqual(compare(report)[0], ['tests/other.phpt (hybrid: failed)'])
        summary = markdown(report)
        self.assertIn(f'1 pending a port (not gated)</summary>\n\n- `{failing} (#{PENDING[failing]}): failed`', summary)
        self.assertIn(f'1 pending a port but passing here: drop them from PENDING_PORTS</summary>\n\n- `{passing}`', summary)

    def test_thread_context_racy_original_runs_without_thread_context(self):
        from corpus.upstream.run import racy, verdict_failed
        case = {'path': 'tests/swoole_coroutine/cancel/throw_exception.phpt'}
        self.assertIsNotNone(racy(case, {'capabilities': ['thread_context']}))
        self.assertIsNone(racy(case, {'capabilities': []}))
        self.assertIsNotNone(racy({'path': 'tests/swoole_server/shutdown_in_master.phpt'}, {'capabilities': []}))
        self.assertTrue(verdict_failed({'test': case['path'], 'status': 'failed'}, True))

    def test_shards_partition_every_path(self):
        paths = [f'tests/t/{n}.phpt' for n in range(25)]
        parts = [shard(paths, index, 4) for index in range(1, 5)]
        self.assertEqual(sorted(p for part in parts for p in part), sorted(paths))
        self.assertTrue(all(abs(len(part) - 25 / 4) < 1 for part in parts))

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

    def test_only_upstream_passes_gate_the_hybrid(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        results = [row('kept', 'upstream', 'passed'), row('kept', 'hybrid', 'passed'),
                   row('lost', 'upstream', 'passed'), row('lost', 'hybrid', 'failed'),
                   row('absent', 'upstream', 'passed'),
                   row('both', 'upstream', 'failed'), row('both', 'hybrid', 'failed'),
                   row('fixed', 'upstream', 'failed'), row('fixed', 'hybrid', 'passed'),
                   row('skip', 'upstream', 'skipped'), row('skip', 'hybrid', 'failed')]
        self.assertEqual(compare(results), (['absent (hybrid: missing)', 'lost (hybrid: failed)'], ['both']))
        self.assertEqual(compare([row('only', 'upstream', 'failed')]), ([], []))
        self.assertIn('2 passing on upstream but not here', markdown(results))

    def test_reruns_separate_regressions_from_flakes(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        results = [row(t, 'upstream', 'passed') for t in ['real', 'upstream-flake', 'hybrid-blip', 'unrun', 'kept']]
        results += [row(t, 'hybrid', 'failed') for t in ['real', 'upstream-flake', 'hybrid-blip', 'unrun']]
        results += [row('kept', 'hybrid', 'passed')]
        self.assertEqual(flagged(results), ['hybrid-blip', 'real', 'unrun', 'upstream-flake'])
        rechecked = [row('real', 'upstream', 'passed')] * 3 + [row('real', 'hybrid', 'passed'), row('real', 'hybrid', 'failed')]
        rechecked += [row('upstream-flake', 'upstream', 'failed'), row('upstream-flake', 'hybrid', 'failed'),
                      row('upstream-flake', 'hybrid', 'passed')]
        rechecked += [row('hybrid-blip', 'upstream', 'passed'), row('hybrid-blip', 'hybrid', 'passed')]
        # 'unrun' has no re-runs, so it stays a regression.
        self.assertEqual(recheck(flagged(results), rechecked),
                         (['real', 'unrun'], ['upstream-flake'], ['hybrid-blip'], []))
        summary = markdown(results, rechecked)
        self.assertIn('2 regressions: pass on upstream, fail on a re-run here', summary)
        self.assertIn('1 flaky on upstream', summary)
        self.assertIn('1 passed on re-run on both engines', summary)
        # The badge counts a hybrid pass on an ungated re-run; a regression's passing re-run doesn't count.
        mark_reruns(results, rechecked)
        self.assertEqual(sorted(r['test'] for r in results if r.get('rerun') == 'passed'), ['hybrid-blip', 'upstream-flake'])

    def test_upstream_flakes_still_gate_a_hybrid_that_always_fails(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        rechecked = [row('broken', 'upstream', 'passed'), row('broken', 'upstream', 'failed')]
        rechecked += [row('broken', 'hybrid', 'failed')] * 3
        rechecked += [row('flaky', 'upstream', 'failed'), row('flaky', 'hybrid', 'failed'), row('flaky', 'hybrid', 'passed')]
        rechecked += [row('unrun', 'upstream', 'failed')]
        # Upstream failing a re-run no longer excuses a hybrid that never passes.
        self.assertEqual(recheck(['broken', 'flaky', 'unrun'], rechecked), (['broken', 'unrun'], ['flaky'], [], []))
        summary = markdown([row(t, 'upstream', 'passed') for t in ['broken', 'flaky']] +
                           [row(t, 'hybrid', 'failed') for t in ['broken', 'flaky']], rechecked)
        self.assertIn('1 regressions', summary)
        self.assertIn('- `flaky`: hybrid failed, passed; upstream failed', summary)

    def test_reruns_stop_once_settled_and_a_timeout_gets_one(self):
        def row(test, engine, status, timed_out=False):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other',
                    'timed_out': timed_out}
        tests = ['flaky-passed', 'flaky-failing', 'steady-failed', 'steady-passing', 'slow']
        rechecked = [row('flaky-passed', 'upstream', 'failed'), row('flaky-passed', 'hybrid', 'passed'),
                     row('flaky-failing', 'upstream', 'failed'), row('flaky-failing', 'hybrid', 'failed'),
                     row('steady-failed', 'upstream', 'passed'), row('steady-failed', 'hybrid', 'failed'),
                     row('steady-passing', 'upstream', 'passed'), row('steady-passing', 'hybrid', 'passed'),
                     row('slow', 'upstream', 'failed'), row('slow', 'hybrid', 'failed', True)]
        results = [row('slow', 'hybrid', 'failed', True)]
        # Upstream stops at its first failure; the hybrid until its verdict is settled.
        self.assertEqual(pending(tests, rechecked, 'upstream'), ['steady-failed', 'steady-passing'])
        self.assertEqual(pending(tests, rechecked, 'hybrid', results), ['flaky-failing', 'steady-failed', 'steady-passing'])
        # Once upstream failed, a test racy on both engines gets nine hybrid re-runs to pass one.
        self.assertEqual(pending(['flaky-failing'], rechecked + [row('flaky-failing', 'hybrid', 'failed')] * 2,
                                 'hybrid', results), ['flaky-failing'])
        self.assertEqual(pending(['flaky-failing'], rechecked + [row('flaky-failing', 'hybrid', 'failed')] * 8,
                                 'hybrid', results), [])
        # One failed re-run doesn't settle a hybrid on a clean upstream; a pass then does (#862),
        # and three failures spend the budget.
        self.assertEqual(pending(['steady-failed'], rechecked + [row('steady-failed', 'hybrid', 'passed')], 'hybrid'), [])
        self.assertEqual(pending(['steady-failed'], rechecked + [row('steady-failed', 'hybrid', 'failed')] * 2, 'hybrid'), [])
        # A first hybrid failure that timed out gets one re-run; timing out again is a regression.
        self.assertEqual(recheck(['slow'], rechecked), (['slow'], [], [], []))
        self.assertEqual(pending(['slow'], [row('slow', 'upstream', 'failed')], 'hybrid', results), ['slow'])

    def test_a_mixed_hybrid_gets_more_upstream_reruns(self):
        def row(test, engine, status):
            return {'test': test, 'family': 'a', 'engine': engine, 'status': status, 'category': 'other'}
        tests = ['mixed', 'broken', 'clean']
        rechecked = [row(t, 'upstream', 'passed') for t in tests for _ in range(3)]
        rechecked += [row('mixed', 'hybrid', 'passed'), row('mixed', 'hybrid', 'failed')]
        rechecked += [row('broken', 'hybrid', 'failed')] * 3 + [row('clean', 'hybrid', 'passed')] * 3
        # Only the hybrid that both passed and failed earns upstream re-runs past three.
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
        # Upstream passed the first run, then failed a re-run; the hybrid failed all nine (#998).
        rechecked = [row('rare', 'upstream', 'failed')] + [row('rare', 'hybrid', 'failed')] * 9
        self.assertEqual(pending(['rare'], rechecked, 'upstream-extended'), ['rare'])
        self.assertEqual(recheck(['rare'], rechecked), (['rare'], [], [], []))
        # Upstream keeps re-running to nine in all, and stops at its first pass: that pass gates it.
        allfail = rechecked + [row('rare', 'upstream', 'failed')] * 8
        self.assertEqual(pending(['rare'], allfail, 'upstream-extended'), [])
        self.assertEqual(recheck(['rare'], allfail), ([], ['rare'], [], []))
        passed = rechecked + [row('rare', 'upstream', 'failed'), row('rare', 'upstream', 'passed')]
        self.assertEqual(pending(['rare'], passed, 'upstream-extended'), [])
        self.assertEqual(recheck(['rare'], passed), (['rare'], [], [], []))

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
        rechecked += [row('replaced', 'hybrid', 'aborted'), row('replaced', 'hybrid', 'passed'),
                      row('replaced', 'hybrid', 'passed'), row('replaced', 'hybrid', 'passed')]
        rechecked += [row('regressed', 'hybrid', 'aborted'), row('regressed', 'hybrid', 'failed')]
        rechecked += [row('never', 'hybrid', 'aborted')] * 6
        rechecked += [row('upstream-only', 'upstream', 'aborted')] * 6 + [row('upstream-only', 'hybrid', 'passed')] * 3
        tests = ['never', 'regressed', 'replaced', 'upstream-only']
        self.assertEqual(recheck(tests, rechecked), (['regressed'], [], ['replaced'], ['never', 'upstream-only']))
        # An inconclusive re-run gets one more in its place, up to twice the budget in all.
        self.assertEqual(pending(['replaced'], rechecked[:3] + [row('replaced', 'hybrid', 'aborted')], 'hybrid'), ['replaced'])
        self.assertEqual(pending(['never'], rechecked, 'hybrid'), [])
        self.assertEqual(pending(['x'], [row('x', 'upstream', 'aborted')] * 2 + [row('x', 'upstream', 'passed')], 'upstream'), ['x'])
        self.assertEqual(pending(['upstream-only'], rechecked, 'upstream'), [])
        summary = markdown([row(t, 'upstream', 'passed') for t in tests] + [row(t, 'hybrid', 'failed') for t in tests], rechecked)
        self.assertIn('2 inconclusive: every re-run on an engine ended in the runner abort', summary)

    def test_incomplete_shards_are_refused(self):
        with tempfile.TemporaryDirectory() as temp:
            report = Path(temp) / 'shard.json'
            report.write_text(json.dumps({'execution_status': 'interrupted', 'results': []}))
            with self.assertRaises(ValueError):
                list(rows([report]))


if __name__ == '__main__':
    unittest.main()
