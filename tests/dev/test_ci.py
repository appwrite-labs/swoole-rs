"""tools/ci.py against a fake gh: run states, failure logs and phases, following replaced runs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

ROOT = Path(__file__).resolve().parents[2]
# Runs are scripted per commit in FAKE_RUNS: {sha: [conclusion, ...]}, one entry
# consumed per `run list`.
FAKE_GH = '''#!/usr/bin/env python3
import json, os, sys, urllib.parse
from pathlib import Path
args = sys.argv[1:]
state = Path(os.environ['FAKE_STATE'])
data = json.loads(state.read_text())
with open(os.environ['FAKE_GH_LOG'], 'a') as log:
    log.write(' '.join(args) + '\\n')
def save():
    state.write_text(json.dumps(data))
call = ' '.join(args)
if call.startswith('api rate_limit --jq .resources.'):
    import time
    print(int(time.time()) + data.get('reset_in', 3600))
    sys.exit(0)
for kind, error in [('flaky', 'HTTP 502: Bad Gateway'), ('limited', 'API rate limit exceeded for user ID 1. (HTTP 403)')]:
    for prefix, count in data.get(kind, {}).items():
        if call.startswith(prefix) and count > 0:
            data[kind][prefix] = count - 1
            save()
            sys.stderr.write(error + '\\n')
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
elif args[0] == 'api' and args[1].startswith('search/issues?q='):
    if data.get('search') is None:
        sys.stderr.write('HTTP 403: API rate limit exceeded\\n')
        sys.exit(1)
    term = urllib.parse.unquote(args[1].split('=', 1)[1]).split('"')[1]
    print(json.dumps(data['search'].get(term, [])))
elif args[0] == 'api' and '/check-runs' in args[1]:
    for check in data.get('checks', {}).get(args[1].split('/')[-2], []):
        print(json.dumps(check))
elif args[:2] == ['api', 'graphql']:
    print(json.dumps(data.get('queue', [])))
elif args[0] == 'api' and '-X' in args:
    pass
elif args[:3] == ['api', '-i', 'user']:
    print('HTTP/2.0 200 OK')
    if 'scopes' in data:
        print('X-Oauth-Scopes: ' + data['scopes'])
    print()
    print('{}')
elif args[0] == 'api' and '/compare/' in args[1]:
    print(data.get('compare', 'ahead'))
elif args[0] == 'api' and '/actions/jobs/' in args[1]:
    print(data['job_logs'][args[1].split('/')[-2]])
elif args[0] == 'api' and '/actions/artifacts?' in args[1]:
    for row in data.get('artifacts', []):
        print(json.dumps({'expired': False, 'head_branch': 'main', 'repository_id': 1, 'head_repository_id': 1, **row}))
elif args[0] == 'api':
    print('c' * 40)
elif args[:2] == ['run', 'list'] and '--commit' not in args:
    print(json.dumps(data.get('history', [])))
elif args[:2] == ['run', 'download'] and '-n' in args:
    report = Path(args[args.index('-D') + 1]) / data['report']
    report.parent.mkdir(parents=True, exist_ok=True)
    report.write_text(json.dumps({'engines': [{'name': 'upstream', 'php': data['php'][args[2]]}]}))
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
    def ci(self, *args, runs, flags=('--poll', '0', '--json'), cwd=None, **extra):
        with tempfile.TemporaryDirectory() as temp:
            gh = Path(temp) / 'gh'
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            state, log = Path(temp) / 'state.json', Path(temp) / 'gh.log'
            state.write_text(json.dumps({'runs': runs, **extra}))
            env = {**os.environ, 'PATH': temp + os.pathsep + os.environ['PATH'],
                   'FAKE_STATE': str(state), 'FAKE_GH_LOG': str(log)}
            result = subprocess.run([sys.executable, ROOT / 'tools/ci.py', *args, *flags],
                                    env=env, capture_output=True, text=True, cwd=cwd)
            self.gh_calls = log.read_text().splitlines() if log.exists() else []
        self.out = result.stdout
        return result.returncode, json.loads(result.stdout) if result.stdout.startswith(('{', '[')) else {}, result.stderr

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
        # Each new failure names the open issues that mention it, so Red main's p0 is filed once.
        self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases, flags=('--poll', '0'),
                search={'http-native': [40, 41]})
        self.assertIn('new failures: http-native (open: #40, #41), ports-added (no open issue); fixed: timer-model',
                      self.out)
        self.assertIn('search/issues?q=%22ports-added%22%20is:issue%20is:open%20repo:{owner}/{repo} '
                      '--jq [.items[].number]', ' '.join(self.gh_calls))
        self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases, flags=('--poll', '0'))
        self.assertIn('http-native (issue search failed), ports-added (issue search failed)', self.out)

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
        jobs = {'7': [['full-gates (1)', 'failure'], ['phpt / gate', 'failure'], ['tooling', 'failure']],
                '4': [['full-gates (1)', 'failure'], ['phpt / gate', 'success'], ['tooling', 'failure']]}
        gate = 'phpt / gate\tRun gate\t2026-10-27T16:02:38.6Z '
        failed_log = '\n'.join(gate + line for line in [
            '<details><summary>2 regressions: pass on upstream, fail on a re-run here</summary>',
            '- `tests/a/one.phpt`', '- `tests/b/two.phpt`',
            '<details><summary>1 flaky on upstream</summary>', '- `tests/c/flaky.phpt`: hybrid passed'])
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases,
                                  jobs=jobs, failed_log=failed_log)
        # full-gates and tooling were already failing; the PHPT gate passed in run 4.
        self.assertEqual(result['new_failures'], ['phpt / gate job (tests/a/one.phpt, tests/b/two.phpt)'])
        # A gate failure's issues are those naming its regressed tests.
        code, result, _ = self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases,
                                  jobs=jobs, failed_log=failed_log, search={'a/one.phpt': [12], 'b/two.phpt': [12, 13]})
        self.assertEqual(result['issues'], {result['new_failures'][0]: [12, 13]})
        # Only the first five tests are searched, and the line says so: an issue may name a later one.
        failed_log = '\n'.join(gate + line for line in [
            '<details><summary>7 regressions: pass on upstream, fail on a re-run here</summary>',
            *[f'- `tests/t/{n}.phpt`' for n in range(7)]])
        self.ci('status', runs={'c' * 40: ['failure']}, history=history, phases=phases, flags=('--poll', '0'),
                jobs=jobs, failed_log=failed_log, search={})
        self.assertIn('.phpt) (no open issue; searched 5 of 7 tests)', self.out)
        self.assertEqual(sum('search/issues' in call for call in self.gh_calls), 5)

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
            return {'number': number, 'title': extra.pop('title', f't{number}'), 'created_at': created,
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
        # A classic token without workflow scope can't do the issues whose title says they need it.
        issues = [issue(1, '2026-10-01T00:00:00Z', 'p1', title='pr.yml: x (needs workflow scope)'),
                  issue(2, '2026-10-02T00:00:00Z', 'p1', title='fuzz: y (needs a token with workflow scope)'),
                  issue(3, '2026-10-03T00:00:00Z', 'p1')]
        for scopes, expected in [('gist, repo', [3]), ('repo, workflow', [1, 2, 3]), (None, [1, 2, 3])]:
            code, result, err = self.ci('next', runs={}, flags=('--json',), issues=issues,
                                        **({'scopes': scopes} if scopes else {}))
            self.assertEqual([row['number'] for row in result], expected, scopes)
            self.assertEqual('hiding 2 issues that need a workflow-scoped token: #1, #2' in err, expected == [3])

    def test_stuck_lists_auto_merge_prs_that_conflict_or_failed_a_required_check(self):
        def pull(number, created, auto=True):
            return {'number': number, 'title': f't{number}', 'created_at': created,
                    'auto_merge': {'merge_method': 'squash'} if auto else None, 'head': {'sha': f'sha{number}'}}
        open_pulls = [pull(1, '2026-11-03T00:00:00Z'), pull(2, '2026-11-01T00:00:00Z'), pull(3, '2026-11-02T00:00:00Z'),
                      pull(4, '2026-10-01T00:00:00Z', auto=False), pull(5, '2026-10-02T00:00:00Z'),
                      pull(6, '2026-11-04T00:00:00Z'), pull(7, '2026-11-05T00:00:00Z')]
        pulls = {str(n): [{'mergeable_state': state}] for n, state in
                 [(1, 'dirty'), (2, 'blocked'), (3, 'clean'), (4, 'dirty'), (5, 'blocked'), (6, 'blocked'), (7, 'blocked')]}
        checks = {'sha1': [], 'sha2': [{'name': 'required', 'conclusion': 'failure'}, {'name': 'rust', 'conclusion': 'failure'}],
                  'sha3': [{'name': 'required', 'conclusion': 'success'}, {'name': 'asan / asan', 'conclusion': 'failure'}],
                  'sha5': [{'name': 'required', 'conclusion': None}], 'sha6': [{'name': 'gates', 'conclusion': 'success'}]}
        # 6's PR-tier run finished without creating `required` (GitHub abandoned its jobs); 7's is still starting.
        runs = {'sha6': ['failure'], 'sha7': ['running']}
        code, result, _ = self.ci('stuck', runs=runs, flags=('--json',), open_pulls=open_pulls, pulls=pulls, checks=checks)
        # 3 only fails an informative check, 4 has no auto-merge and 5 is still running.
        self.assertEqual((code, [(row['number'], row['blocking']) for row in result]),
                         (0, [(2, ['required']), (1, ['conflicts']), (6, ['no required check'])]))
        code, result, _ = self.ci('stuck', runs={}, flags=('--json',), open_pulls=[])
        self.assertEqual((code, result), (2, []))

    def test_stuck_lists_prs_the_merge_queue_dropped_with_their_group_run(self):
        def pull(number, created, events, queued=False, auto=None):
            return {'number': number, 'title': f't{number}', 'createdAt': created, 'isDraft': False, 'isInMergeQueue': queued,
                    'autoMergeRequest': auto, 'timelineItems': {'nodes': events}}
        removed = [{'__typename': 'RemovedFromMergeQueueEvent', 'reason': 'failed status checks'}]
        queue = [pull(8, '2026-10-15T00:00:00Z', removed), pull(9, '2026-10-16T00:00:00Z', [{'__typename': 'AddedToMergeQueueEvent'}], queued=True),
                 pull(10, '2026-10-17T00:00:00Z', []), pull(11, '2026-10-18T00:00:00Z', removed, auto={'enabledAt': 'x'}),
                 pull(12, '2026-10-19T00:00:00Z', [{'__typename': 'RemovedFromMergeQueueEvent', 'reason': None}])]
        history = [{'headBranch': 'gh-readonly-queue/main/pr-8-abc', 'url': 'u8'}, {'headBranch': 'main', 'url': 'm'}]
        code, result, _ = self.ci('stuck', runs={}, flags=('--json',), open_pulls=[], queue=queue, history=history)
        # 9 is queued, 10 never was, and 11 was queued again: its auto-merge waits on a new PR-tier run.
        self.assertEqual((code, [(row['number'], row['blocking']) for row in result]),
                         (0, [(8, ['left the merge queue: failed status checks', 'u8']),
                              (12, ['left the merge queue: no reason given'])]))

    def test_a_merge_queue_run_may_fail_only_what_main_already_fails(self):
        history = [{'databaseId': 9, 'conclusion': 'cancelled', 'url': 'u9'}, {'databaseId': 4, 'conclusion': 'failure', 'url': 'u4'}]
        # Shard 1 reports a known failure and a regression, shard 2 passes and shard 3 died before its gates.
        phases = {'7': [[['old', False], ['timer-model', False]], [['build', True]]], '4': [['old', False], ['timer-model', True]]}
        jobs = {'7': [['full-gates (1)', 'failure'], ['full-gates (2)', 'success'], ['full-gates (3)', 'failure'],
                      ['callgrind', 'failure'], ['phpt / gate', 'failure'], ['alpine', 'cancelled'], ['required', None]],
                '4': [['full-gates (1)', 'failure'], ['callgrind', 'failure'], ['phpt / gate', 'success'], ['alpine', 'success']]}
        code, _, _ = self.ci('new-failures', '--run', '7', runs={}, flags=(), history=history, phases=phases, jobs=jobs)
        self.assertEqual(code, 1)
        # callgrind fails on main too, and the run's own unfinished `required` job isn't judged.
        self.assertIn('Against u4: new failures: timer-model, alpine job, full-gates (3) job (no phases), phpt / gate job', self.out)
        # A finished run's log is never read: the run is still in progress.
        self.assertFalse(any('--log-failed' in call or 'search/issues' in call for call in self.gh_calls))
        phases['7'] = [['old', False]]
        jobs['7'] = [['full-gates (1)', 'failure'], ['callgrind', 'failure'], ['required', None]]
        code, _, _ = self.ci('new-failures', '--run', '7', runs={}, flags=(), history=history, phases=phases, jobs=jobs)
        self.assertEqual(code, 0)
        self.assertIn('no new failures', self.out)

    def test_new_failures_are_gate_failures_main_does_not_share(self):
        history = [{'databaseId': 9, 'conclusion': 'cancelled', 'url': 'u9'},
                   {'databaseId': 8, 'conclusion': 'failure', 'url': 'u8'}]
        main_phases = {'8': [['build', True], ['abi-native', False], ['timer-native', True]]}
        with tempfile.TemporaryDirectory() as temp:
            loop = Path(temp) / 'loop.json'
            loop.write_text(json.dumps({'phases': [{'phase': 'build', 'failed': False},
                                                   {'phase': 'abi-native', 'failed': True},
                                                   {'phase': 'timer-native', 'failed': True},
                                                   {'phase': 'added-native', 'failed': True}]}))
            code, _, _ = self.ci('new-failures', str(loop), runs={}, flags=(), history=history, phases=main_phases)
            self.assertEqual(code, 1)
            # abi-native fails on main too; timer-native regressed and added-native is new.
            self.assertIn('new failures: added-native, timer-native', self.out)
            loop.write_text(json.dumps({'phases': [{'phase': 'abi-native', 'failed': True}]}))
            code, _, _ = self.ci('new-failures', str(loop), runs={}, flags=(), history=history, phases=main_phases)
            self.assertEqual(code, 0)
        code, _, _ = self.ci('new-failures', runs={}, flags=())
        self.assertEqual(code, 1)

    def test_phpt_baseline_is_the_newest_run_whose_upstream_shards_passed_while_its_key_holds(self):
        with tempfile.TemporaryDirectory() as repo:
            def git(*args):
                return subprocess.run(['git', '-C', repo, '-c', 'user.name=t', '-c', 'user.email=t@t', *args],
                                      check=True, capture_output=True, text=True).stdout.strip()

            def commit(path, text):
                (Path(repo) / path).parent.mkdir(parents=True, exist_ok=True)
                (Path(repo) / path).write_text(text)
                git('add', '-A')
                git('commit', '-qm', path)
                return git('rev-parse', 'HEAD')
            git('init', '-q')
            old = commit('tools/build_upstream.py', 'pin 1')
            base = commit('README.md', 'unrelated')
            # A fork's branch named main, a shard still running, a shard an outage dropped: none of
            # them stands. A failed cargo shard doesn't matter.
            artifacts = [{'id': 11, 'head_sha': base, 'head_repository_id': 2}, {'id': 10, 'head_sha': base},
                         {'id': 9, 'head_sha': base}, {'id': 8, 'head_sha': base}, {'id': 8, 'head_sha': base},
                         {'id': 7, 'head_sha': old}]
            shards = [[f'phpt / phpt ({index}, upstream)', 'success'] for index in range(1, 7)]
            jobs = {'11': shards, '10': shards[:5] + [['phpt / phpt (6, upstream)', '']], '9': shards[1:],
                    '8': shards + [['phpt / phpt (1, cargo)', 'failure'], ['phpt / gate', 'failure']], '7': shards}
            php = {'8': '8.3.6'}
            state = {'runs': {}, 'flags': (), 'cwd': repo, 'artifacts': artifacts, 'jobs': jobs, 'php': php,
                     'report': 'upstream-1.json'}
            code, _, _ = self.ci('phpt-baseline', **state)
            self.assertEqual((code, self.out), (0, f'8 {base}\n'))
            self.assertEqual(sum(call.startswith('run view 8 ') for call in self.gh_calls), 1)
            code, _, _ = self.ci('phpt-baseline', '--php', '8.3.6', **state)
            self.assertEqual((code, self.out), (0, f'8 {base}\n'))
            # Another PHP release, or a changed key path since the baseline: upstream runs again.
            code, _, _ = self.ci('phpt-baseline', '--php', '8.3.7', **state)
            self.assertEqual((code, self.out), (2, ''))
            for path, text in [('.github/actions/upstream/action.yml', 'builds upstream'), ('tools/build_upstream.py', 'pin 2')]:
                git('checkout', '-q', base)
                commit(path, text)
                code, _, _ = self.ci('phpt-baseline', **state)
                self.assertEqual((code, self.out), (2, ''), path)
            # A variant's own workflow is part of its key, and its one job makes its report.
            variant = {**state, 'jobs': {'8': [['cares / phpt', 'success']]}, 'report': 'target/phpt/cares/upstream.json'}
            git('checkout', '-q', base)
            code, _, _ = self.ci('phpt-baseline', '--variant', 'cares', '--php', '8.3.6', **variant)
            self.assertEqual((code, self.out), (0, f'8 {base}\n'))
            self.assertIn('run download 8 -n phpt-cares', ' '.join(self.gh_calls))
            commit('.github/workflows/cares.yml', 'families')
            code, _, _ = self.ci('phpt-baseline', '--variant', 'cares', **variant)
            self.assertEqual((code, self.out), (2, ''))
            # ZTS's report comes from its own artifact, and the thread-safe PHP it runs on
            # (.devcontainer/php.sh) is part of its key.
            zts = {**state, 'jobs': {'8': [['zts / phpt', 'success']]}, 'report': 'target/phpt/zts/upstream.json'}
            git('checkout', '-q', base)
            code, _, _ = self.ci('phpt-baseline', '--variant', 'zts', '--php', '8.3.6', **zts)
            self.assertEqual((code, self.out), (0, f'8 {base}\n'))
            self.assertIn('run download 8 -n phpt-zts', ' '.join(self.gh_calls))
            commit('.devcontainer/php.sh', 'another PHP')
            code, _, _ = self.ci('phpt-baseline', '--variant', 'zts', **zts)
            self.assertEqual((code, self.out), (2, ''))

    def test_transient_gh_failures_are_retried(self):
        code, result, stderr = self.ci('wait', runs={'c' * 40: ['success']}, flaky={'run list': 2, 'api': 1})
        self.assertEqual((code, result['state']), (0, 'passed'))
        self.assertEqual(stderr.count('HTTP 502: Bad Gateway; retrying'), 3)

    def test_an_exhausted_rate_limit_is_waited_out_within_the_deadline_and_named_beyond_it(self):
        code, result, stderr = self.ci('wait', runs={'c' * 40: ['success']}, limited={'run list': 1}, reset_in=0)
        self.assertEqual((code, result['state']), (0, 'passed'))
        self.assertIn('API rate limit exceeded; waiting until', stderr)
        # `next` has no deadline: it stops at once and says when the limit resets.
        code, _, stderr = self.ci('next', runs={}, flags=(), limited={'api': 1})
        self.assertEqual(code, 1)
        self.assertRegex(stderr, r'API rate limit exceeded .*; it resets at \d\d:\d\d:\d\d')
        self.assertIn('api rate_limit --jq .resources.core.reset', self.gh_calls)

    def test_a_search_call_waits_on_the_search_limit(self):
        sys.path.insert(0, str(ROOT))
        from tools import ci
        with tempfile.TemporaryDirectory() as temp:
            gh = Path(temp) / 'gh'
            gh.write_text(FAKE_GH)
            gh.chmod(0o755)
            state, log = Path(temp) / 'state.json', Path(temp) / 'gh.log'
            state.write_text(json.dumps({'runs': {}, 'limited': {'api search/issues': 1}, 'reset_in': 0, 'search': {}}))
            env = {'PATH': temp + os.pathsep + os.environ['PATH'], 'FAKE_STATE': str(state), 'FAKE_GH_LOG': str(log)}
            with unittest.mock.patch.dict(os.environ, env), unittest.mock.patch.object(ci, 'RETRY_DEADLINE', float('inf')):
                ci.gh('api', 'search/issues?q=%22x%22')
            self.assertIn('api rate_limit --jq .resources.search.reset', log.read_text().splitlines())

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
