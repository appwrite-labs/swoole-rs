"""tools/ci.py against a fake gh: run states, failure logs and phases, following replaced runs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
# Runs are scripted per commit in FAKE_RUNS: {sha: [conclusion, ...]}, one entry
# consumed per `run list`.
FAKE_GH = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
args = sys.argv[1:]
state = Path(os.environ['FAKE_STATE'])
data = json.loads(state.read_text())
with open(os.environ['FAKE_GH_LOG'], 'a') as log:
    log.write(' '.join(args) + '\\n')
def save():
    state.write_text(json.dumps(data))
call = ' '.join(args)
for prefix, count in data.get('flaky', {}).items():
    if call.startswith(prefix) and count > 0:
        data['flaky'][prefix] = count - 1
        save()
        sys.stderr.write('HTTP 502: Bad Gateway\\n')
        sys.exit(1)
deleted = [branch for branch in data.get('deleted', []) if call.startswith('api repos/{owner}/{repo}/commits/' + branch)]
if deleted:
    sys.stderr.write(f'gh: No commit found for SHA: {deleted[0]} (HTTP 422)\\n')
    sys.exit(1)
elif args[:2] == ['api', '--paginate']:
    for issue in data.get('open_pulls' if '/pulls?' in args[2] else 'issues', []):
        print(json.dumps(issue))
elif args[0] == 'api' and args[1].endswith('/pulls'):
    print(json.dumps(data.get('commit_pulls', [])))
elif args[0] == 'api' and '/pulls?' in args[1]:
    print('e' * 40)
elif args[0] == 'api' and '/pulls/' in args[1]:
    queue = data['pulls'][args[1].rsplit('/', 1)[1]]
    pull = queue.pop(0) if len(queue) > 1 else queue[0]
    save()
    print(json.dumps({'html_url': 'p', 'merged': False, 'merge_commit_sha': None, **pull}))
elif args[0] == 'api' and '/check-runs' in args[1]:
    for check in data.get('checks', {}).get(args[1].split('/')[-2], []):
        print(json.dumps(check))
elif args[0] == 'api' and '-X' in args:
    pass
elif args[0] == 'api' and '/compare/' in args[1]:
    print(data.get('compare', 'ahead'))
elif args[0] == 'api' and '/actions/jobs/' in args[1]:
    print(data['job_logs'][args[1].split('/')[-2]])
elif args[0] == 'api':
    print('c' * 40)
elif args[:2] == ['run', 'list'] and '--commit' not in args:
    print(json.dumps(data.get('history', [])))
elif args[:2] == ['run', 'download']:
    rows = data.get('phases', {}).get(args[2])
    if rows is None:
        sys.exit(1)
    # A run's phases are one list, or one list per full-gates shard (artifacts phases-1, phases-2, ...).
    shards = rows if rows and isinstance(rows[0][0], list) else [rows]
    for index, shard in enumerate(shards, 1):
        loop = Path(args[args.index('-D') + 1]) / f'phases-{index}/target/reports/dev/x/loop.json'
        loop.parent.mkdir(parents=True)
        loop.write_text(json.dumps({'phases': [{'phase': row[0], 'failed': not row[1], **({'log_tail': row[2]} if len(row) > 2 else {})}
                                               for row in shard]}))
elif args[:2] == ['run', 'list']:
    sha = args[args.index('--commit') + 1]
    queue = data['runs'].get(sha, [])
    conclusion = queue.pop(0) if len(queue) > 1 else (queue[0] if queue else None)
    save()
    if conclusion is not None:
        running = conclusion == 'running'
        print(json.dumps([{'databaseId': 7, 'status': 'in_progress' if running else 'completed',
                           'conclusion': '' if running else conclusion, 'url': 'u'}]))
    else:
        print('[]')
elif args[:2] == ['run', 'view'] and '--log-failed' in args:
    print(data.get('failed_log', 'rust\\tcargo test\\t2026-10-25T21:00:01Z error: boom'))
elif args[:2] == ['run', 'view']:
    jobs = data.get('jobs', {}).get(args[2], [['rust', 'success']])
    print(json.dumps({'jobs': [{'name': name, 'databaseId': index + 100, 'status': 'completed', 'conclusion': conclusion,
                                'startedAt': '2026-10-25T21:00:00Z', 'completedAt': '2026-10-25T21:00:31Z'}
                               for index, (name, conclusion) in enumerate(jobs)]}))
'''


class Ci(unittest.TestCase):
    def ci(self, *args, runs, flags=('--poll', '0', '--json'), **extra):
        with tempfile.TemporaryDirectory() as temp:
            gh = Path(temp) / 'gh'
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            state, log = Path(temp) / 'state.json', Path(temp) / 'gh.log'
            state.write_text(json.dumps({'runs': runs, **extra}))
            env = {**os.environ, 'PATH': temp + os.pathsep + os.environ['PATH'],
                   'FAKE_STATE': str(state), 'FAKE_GH_LOG': str(log)}
            result = subprocess.run([sys.executable, ROOT / 'tools/ci.py', *args, *flags],
                                    env=env, capture_output=True, text=True)
            self.gh_calls = log.read_text().splitlines() if log.exists() else []
        return result.returncode, json.loads(result.stdout or '{}'), result.stderr

    def test_wait_defaults_to_the_merge_tier_on_main(self):
        code, result, stderr = self.ci('wait', runs={'c' * 40: ['running', 'success']})
        self.assertEqual((code, result['workflow'], result['state']), (0, 'merge.yml', 'passed'))
        self.assertIn('api repos/{owner}/{repo}/commits/main -q .sha', self.gh_calls)
        self.assertIn('rust: success in 31s', stderr)

    def test_wait_follows_the_run_that_replaced_its_commit(self):
        history = [{'headSha': 'd' * 40}]
        code, result, stderr = self.ci('wait', runs={'c' * 40: ['cancelled'], 'd' * 40: ['success']}, history=history)
        self.assertEqual((code, result['commit'], result['state']), (0, 'd' * 40, 'passed'))
        self.assertIn('replaced by the run on ddddddd', stderr)
        code, result, _ = self.ci('wait', runs={'c' * 40: ['cancelled']}, history=history, compare='diverged')
        self.assertEqual((code, result['commit'], result['state']), (3, 'c' * 40, 'cancelled'))

    def test_merge_tier_failure_lists_failed_new_and_fixed_phases(self):
        history = [{'databaseId': 7, 'conclusion': 'failure', 'url': 'u7'}, {'databaseId': 6, 'conclusion': 'cancelled', 'url': 'u6'},
                   {'databaseId': 5, 'conclusion': 'failure', 'url': 'u5'}, {'databaseId': 4, 'conclusion': 'failure', 'url': 'u4'}]
        phases = {'7': [['build', False], ['timer-model', True], ['http-native', False], ['tls-native', True],
                        ['ports-added', False]],
                  '4': [['build', False], ['timer-model', False], ['http-native', True]]}
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases)
        self.assertEqual((code, result['failed_phases']), (1, ['build', 'http-native', 'ports-added']))
        # Run 6 was cancelled and run 5 has no phases artifact, so run 4 is the comparison.
        # ports-added didn't exist in run 4, so its failure is new as well.
        self.assertEqual((result['previous_run'], result['new_failures'], result['fixed']),
                         ('u4', ['http-native', 'ports-added'], ['timer-model']))
        self.assertEqual(result['logs'], {'rust': ['2026-10-25T21:00:01Z error: boom']})

    def test_shard_artifacts_are_merged(self):
        history = [{'databaseId': 7, 'conclusion': 'failure', 'url': 'u7'}, {'databaseId': 4, 'conclusion': 'failure', 'url': 'u4'}]
        phases = {'7': [[['build', True], ['timer-model', False]], [['build', False], ['http-native', True]]],
                  '4': [[['build', True], ['timer-model', True]], [['build', True], ['http-native', True]]]}
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases)
        # build passed in one shard and failed in the other, so it failed.
        self.assertEqual(result['failed_phases'], ['build', 'timer-model'])
        self.assertEqual(result['new_failures'], ['build', 'timer-model'])

    def test_new_failures_bring_their_log_tails(self):
        history = [{'databaseId': 7, 'conclusion': 'failure', 'url': 'u7'}, {'databaseId': 4, 'conclusion': 'failure', 'url': 'u4'}]
        phases = {'7': [['old', False, ['still broken']], ['new', False, ['first line', 'why it broke']]],
                  '4': [['old', False], ['new', True]]}
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases)
        self.assertEqual(result['new_failures'], ['new'])
        self.assertEqual(result['phase_logs'], {'new': ['first line', 'why it broke']})

    def test_a_job_failing_without_failed_phases_is_new_when_it_passed_before(self):
        history = [{'databaseId': 7, 'conclusion': 'failure', 'url': 'u7'}, {'databaseId': 4, 'conclusion': 'failure', 'url': 'u4'}]
        phases = {'7': [['old', False]], '4': [['old', False]]}
        jobs = {'7': [['full-gates (1)', 'failure'], ['phpt-gate', 'failure'], ['tooling', 'failure']],
                '4': [['full-gates (1)', 'failure'], ['phpt-gate', 'success'], ['tooling', 'failure']]}
        gate = 'phpt-gate\tRun gate\t2026-10-27T16:02:38.6Z '
        failed_log = '\n'.join(gate + line for line in [
            '<details><summary>2 regressions: pass on upstream, fail on a re-run here</summary>',
            '- `tests/a/one.phpt`', '- `tests/b/two.phpt`',
            '<details><summary>1 flaky on upstream</summary>', '- `tests/c/flaky.phpt`: hybrid passed'])
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases,
                                  jobs=jobs, failed_log=failed_log)
        # full-gates and tooling were already failing; phpt-gate passed in run 4.
        self.assertEqual(result['new_failures'], ['phpt-gate job (tests/a/one.phpt, tests/b/two.phpt)'])

    def test_full_gates_shards_speak_through_their_phases_and_unseen_jobs_are_new(self):
        history = [{'databaseId': 7, 'conclusion': 'failure', 'url': 'u7'}, {'databaseId': 4, 'conclusion': 'success', 'url': 'u4'}]
        phases = {'7': [['timer-model', False]], '4': [['timer-model', True]]}
        jobs = {'7': [['full-gates (1)', 'failure'], ['brand-new', 'failure']], '4': [['full-gates (1)', 'success']]}
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases, jobs=jobs)
        # One breakage, one entry: the shard's failure is its phase; a job the previous run
        # didn't have is new, as a phase would be.
        self.assertEqual(result['new_failures'], ['timer-model', 'brand-new job'])

    def test_runs_without_phases_keep_the_log_tail(self):
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']})
        self.assertNotIn('failed_phases', result)
        self.assertIn('rust', result['logs'])

    def test_next_orders_claimable_issues_by_priority_then_age(self):
        def issue(number, created, *labels, blocked_by=0, **extra):
            return {'number': number, 'title': f't{number}', 'created_at': created,
                    'labels': [{'name': name} for name in ('status:ready', *labels)],
                    'issue_dependencies_summary': {'blocked_by': blocked_by}, **extra}
        issues = [issue(1, '2026-10-01T00:00:00Z', 'p2'), issue(2, '2026-10-05T00:00:00Z', 'p1'),
                  issue(3, '2026-10-02T00:00:00Z', 'p1'), issue(4, '2026-10-09T00:00:00Z', 'p0'),
                  issue(5, '2026-09-01T00:00:00Z'), issue(6, '2026-09-01T00:00:00Z', 'p0', blocked_by=1),
                  issue(7, '2026-09-01T00:00:00Z', 'p0', 'needs:decision'),
                  issue(8, '2026-09-01T00:00:00Z', 'p0', pull_request={})]
        code, result, _ = self.ci('next', runs={}, flags=('--json',), issues=issues)
        self.assertEqual((code, [row['number'] for row in result]), (0, [4, 3, 2, 1, 5]))
        # REST, not `gh issue list`: GraphQL is unavailable in some agent sessions.
        self.assertIn('api --paginate repos/{owner}/{repo}/issues?state=open&labels=status:ready&per_page=100 '
                      '--jq .[]', self.gh_calls)
        code, result, _ = self.ci('next', runs={}, flags=('--json',), issues=[])
        self.assertEqual((code, result), (2, []))

    def test_stuck_lists_auto_merge_prs_that_conflict_or_failed_a_required_check(self):
        def pull(number, created, auto=True):
            return {'number': number, 'title': f't{number}', 'created_at': created,
                    'auto_merge': {'merge_method': 'squash'} if auto else None, 'head': {'sha': f'sha{number}'}}
        open_pulls = [pull(1, '2026-11-03T00:00:00Z'), pull(2, '2026-11-01T00:00:00Z'), pull(3, '2026-11-02T00:00:00Z'),
                      pull(4, '2026-10-01T00:00:00Z', auto=False), pull(5, '2026-10-02T00:00:00Z')]
        pulls = {str(n): [{'mergeable_state': state}] for n, state in
                 [(1, 'dirty'), (2, 'blocked'), (3, 'clean'), (4, 'dirty'), (5, 'blocked')]}
        checks = {'sha1': [], 'sha2': [{'name': 'hybrid', 'conclusion': 'failure'}, {'name': 'rust', 'conclusion': 'timed_out'},
                                       {'name': 'gates', 'conclusion': 'failure'}],
                  'sha3': [{'name': 'rust', 'conclusion': 'success'}, {'name': 'gates', 'conclusion': 'failure'}],
                  'sha5': [{'name': 'tooling', 'conclusion': None}]}
        code, result, _ = self.ci('stuck', runs={}, flags=('--json',), open_pulls=open_pulls, pulls=pulls, checks=checks)
        # 3 only fails the informative gates, 4 has no auto-merge and 5 is still running.
        self.assertEqual((code, [(row['number'], row['blocking']) for row in result]),
                         (0, [(2, ['hybrid', 'rust']), (1, ['conflicts'])]))
        code, result, _ = self.ci('stuck', runs={}, flags=('--json',), open_pulls=[])
        self.assertEqual((code, result), (2, []))

    def test_transient_gh_failures_are_retried(self):
        code, result, stderr = self.ci('wait', runs={'c' * 40: ['success']}, flaky={'run list': 2, 'api': 1})
        self.assertEqual((code, result['state']), (0, 'passed'))
        self.assertEqual(stderr.count('HTTP 502: Bad Gateway; retrying'), 3)

    def test_a_deleted_branch_resolves_through_its_pull_request(self):
        code, result, _ = self.ci('wait', '--workflow', 'pr.yml', '--commit', 'socket-timeout',
                                  runs={'e' * 40: ['success']}, deleted=['socket-timeout'])
        self.assertEqual((code, result['commit'], result['state']), (0, 'e' * 40, 'passed'))
        self.assertIn('api repos/{owner}/{repo}/pulls?state=all&head={owner}:socket-timeout -q .[0].head.sha // ""',
                      self.gh_calls)
        # The 422 is permanent, though the branch name contains 'timeout': resolved once, not retried.
        self.assertNotIn('retrying', _)

    def test_a_conflicting_pr_returns_at_once(self):
        pulls = {'9': [{'state': 'open', 'mergeable_state': 'dirty'}]}
        # The PR tier never starts for a conflicting head: no five-minute wait for it to appear.
        code, result, stderr = self.ci('wait', '--workflow', 'pr.yml', '--commit', 'a' * 40, runs={},
                                       pulls=pulls, commit_pulls=[9])
        self.assertEqual((code, result['state'], result['pr']), (4, 'conflicts', 9))
        self.assertIn('PR #9 conflicts with main', stderr)

    def test_status_without_a_run_is_none(self):
        code, result, _ = self.ci('status', '--commit', 'abc1234', runs={})
        self.assertEqual((code, result['state']), (2, 'none'))

    def test_unfinished_head_shows_completed_failures_without_changing_its_state(self):
        history = [{'databaseId': 6, 'conclusion': 'cancelled', 'url': 'u6', 'headSha': 'f' * 40},
                   {'databaseId': 5, 'conclusion': 'failure', 'status': 'completed', 'url': 'u5', 'headSha': 'e' * 40},
                   {'databaseId': 4, 'conclusion': 'cancelled', 'url': 'u4', 'headSha': 'd' * 40},
                   {'databaseId': 3, 'conclusion': 'success', 'url': 'u3', 'headSha': 'b' * 40}]
        phases = {'5': [['socket', False, ['actual failure']]], '3': [['socket', True]]}
        jobs = {'5': [['full-gates', 'failure']], '3': [['full-gates', 'success']]}
        for conclusion, state, expected_code in [('running', 'running', 2), (None, 'none', 2),
                                                  ('cancelled', 'cancelled', 3)]:
            with self.subTest(conclusion=conclusion):
                code, result, _ = self.ci('status', runs={'c' * 40: [] if conclusion is None else [conclusion]},
                                          history=history, phases=phases, jobs=jobs)
                self.assertEqual((code, result['state'], result['commit']), (expected_code, state, 'c' * 40))
                completed = result['last_completed']
                self.assertEqual((completed['commit'], completed['url'], completed['state']), ('e' * 40, 'u5', 'failed'))
                self.assertEqual((completed['previous_run'], completed['new_failures']), ('u3', ['socket']))
                self.assertEqual(completed['phase_logs'], {'socket': ['actual failure']})

    def test_pr_status_does_not_query_main_history(self):
        code, result, _ = self.ci('status', '--workflow', 'pr.yml', runs={'c' * 40: ['running']})
        self.assertEqual(code, 2)
        self.assertNotIn('last_completed', result)
        self.assertFalse(any('--branch main' in call for call in self.gh_calls))

    def test_failed_job_logs_are_visible_while_siblings_run(self):
        log = '\n'.join(['old output'] * 50 + ['compiler diagnostic', '##[error]Process completed with exit code 1.']
                        + ['artifact upload and cleanup'] * 60)
        code, result, _ = self.ci('status', '--workflow', 'pr.yml', runs={'c' * 40: ['running']},
                                  jobs={'7': [['hybrid', 'failure'], ['cares', '']]}, job_logs={'100': log})
        self.assertEqual((code, result['state']), (2, 'running'))
        self.assertEqual(len(result['logs']['hybrid']), 40)
        self.assertEqual(result['logs']['hybrid'][-2:], ['compiler diagnostic', '##[error]Process completed with exit code 1.'])
        self.assertNotIn('artifact upload and cleanup', result['logs']['hybrid'])
        self.assertEqual([call for call in self.gh_calls if '/actions/jobs/' in call],
                         ['api repos/{owner}/{repo}/actions/jobs/100/logs --allow-escape-sequences'])


if __name__ == '__main__':
    unittest.main()
