"""Run history keeps failed attempts visible when a rerun is active or passes."""
import contextlib
import io
import json
import subprocess
import unittest
from unittest.mock import patch

from tools import ci


def run(number, attempt=1):
    return dict(databaseId=number, attempt=attempt, headSha=f'sha{number}', headBranch='topic',
                event='pull_request', url=f'u{number}', createdAt=f'2026-11-16T03:{number:02}:00Z',
                startedAt=f'2026-11-16T03:{number:02}:10Z', updatedAt='2026-11-16T04:00:00Z',
                status='completed', conclusion='success')


class History(unittest.TestCase):
    def test_cross_branch_runs_keep_failed_attempts_even_during_a_rerun(self):
        for status, conclusion in [('in_progress', ''), ('completed', 'success')]:
            latest = run(9, 2)
            latest.update(status=status, conclusion=conclusion)
            queue = run(8)
            queue.update(headBranch='gh-readonly-queue/main/pr-8-base', event='merge_group', conclusion='cancelled')
            prior = dict(id=9, head_sha='sha9', head_branch='topic', html_url='u9', event='pull_request',
                         run_attempt=1, created_at=latest['createdAt'], run_started_at=latest['startedAt'],
                         updated_at='2026-11-16T03:30:00Z', status='completed', conclusion='failure')
            def gh(*args):
                if args[0] == 'api':
                    self.assertEqual(args[1], 'repos/{owner}/{repo}/actions/runs/9/attempts/1')
                    return json.dumps(prior)
                self.assertNotIn('--branch', args)
                self.assertNotIn('--status', args)
                self.assertNotIn('--event', args)
                self.assertEqual(args[args.index('--created') + 1], '>=2026-11-16T03:00:00Z')
                self.assertEqual(args[args.index('--limit') + 1], '1')
                return json.dumps([latest] if args[args.index('--workflow') + 1] == 'pr.yml' else [queue])
            with self.subTest(status=status), patch.object(ci, 'gh', side_effect=gh) as calls:
                result = ci.history(['pr.yml', 'merge.yml', 'pr.yml'], 1, '2026-11-16T03:00:00Z')
                self.assertEqual(result['limit_reached'], ['pr.yml', 'merge.yml'])
                self.assertEqual([(r['databaseId'], r['attempt'], r['conclusion']) for r in result['runs']],
                                 [(8, 1, 'cancelled'), (9, 1, 'failure'), (9, 2, conclusion)])
                self.assertEqual(result['runs'][1]['updatedAt'], prior['updated_at'])
                self.assertEqual(result['runs'][2]['status'], status)
                self.assertEqual(result['runs'][0]['headBranch'], queue['headBranch'])
                self.assertEqual(calls.call_count, 3)

    def test_cli_empty_history_and_invalid_limits(self):
        with patch.object(ci, 'gh', return_value='[]') as calls, contextlib.redirect_stdout(io.StringIO()) as out:
            self.assertEqual(ci.main(['history', '--workflow', 'merge.yml']), 2)
            result = json.loads(out.getvalue())
            self.assertEqual(result, {'limit_per_workflow': 30, 'since': None, 'limit_reached': [], 'runs': []})
            self.assertNotIn('--created', calls.call_args.args)
        for limit in ['0', '-1', '1001']:
            with self.subTest(limit=limit), patch.object(ci, 'gh') as calls, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    ci.main(['history', '--workflow', 'pr.yml', '--limit', limit])
                self.assertEqual(error.exception.code, 2)
                calls.assert_not_called()

    def test_unavailable_attempt_is_an_error_not_silently_dropped(self):
        failure = subprocess.CalledProcessError(1, ['gh', 'api', 'attempt'], stderr='HTTP 404')
        with patch.object(ci, 'gh', side_effect=[json.dumps([run(9, 2)]), failure]), \
                contextlib.redirect_stdout(io.StringIO()) as out:
            with self.assertRaises(SystemExit) as error:
                ci.main(['history', '--workflow', 'pr.yml'])
            self.assertIn('HTTP 404', str(error.exception))
            self.assertEqual(out.getvalue(), '')


if __name__ == '__main__':
    unittest.main()
