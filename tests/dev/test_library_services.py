"""Library service startup preserves failures and only retries the observed Oracle race."""
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from corpus.library import run

ORACLE_FAILURE = """ORA-00442: Oracle Database Express Edition (XE) single instance violation error
ORA-27300: OS system dependent operation:read failed with status: 3
ORA-27301: OS failure message: No such process
ORA-27302: failure occurred at: sxecheck2
"""


class LibraryServices(unittest.TestCase):
    def start(self, stopped, log=ORACLE_FAILURE, ready=True, recreate=0):
        evidence = Path('target/reports/library-test/services')
        def compose(*args):
            return subprocess.CompletedProcess(args, recreate if args[0] == 'up' else 0,
                                               stdout=log, stderr='')
        with patch.object(run.subprocess, 'run'), patch.object(run, 'stopped', side_effect=stopped), \
                patch.object(run, 'ready', return_value=ready) as probe, \
                patch.object(run, 'compose', side_effect=compose) as commands, \
                patch.object(run, 'keep_evidence') as keep, patch.object(run.time, 'sleep'):
            result = run.start_services(evidence, attempts=len(stopped), interval=0)
        return result, commands, keep, probe

    def test_recovers_once_before_testing_readiness(self):
        result, commands, keep, probe = self.start([['oracle'], []])
        self.assertTrue(result)
        self.assertEqual(probe.call_count, 1)
        commands.assert_any_call('up', '-d', '--no-deps', '--force-recreate',
                                 '--renew-anon-volumes', 'oracle')
        keep.assert_called_once_with(Path('target/reports/library-test/services/oracle-first-start'))

    def test_repeated_failure_is_not_ready_even_with_old_ready_log(self):
        result, commands, keep, probe = self.start([['oracle'], ['oracle']])
        self.assertFalse(result)
        probe.assert_not_called()
        self.assertEqual(sum(c.args[0] == 'up' for c in commands.call_args_list), 1)
        self.assertEqual(keep.call_count, 2)

    def test_other_failures_do_not_retry(self):
        for stopped, log in [(['mysql'], ORACLE_FAILURE), (['mysql', 'oracle'], ORACLE_FAILURE),
                             (['oracle'], 'ORA-00442: different cause')]:
            with self.subTest(stopped=stopped, log=log):
                result, commands, keep, probe = self.start([stopped], log)
                self.assertFalse(result)
                self.assertFalse(any(c.args[0] == 'up' for c in commands.call_args_list))
                probe.assert_not_called()
                keep.assert_called_once()

    def test_failed_recreation_and_readiness_timeout_fail(self):
        for stopped, ready, recreate in [([['oracle']], True, 1), ([[], []], False, 0)]:
            with self.subTest(stopped=stopped):
                result, _, keep, _ = self.start(stopped, ready=ready, recreate=recreate)
                self.assertFalse(result)
                self.assertEqual(keep.call_args.args, (Path('target/reports/library-test/services'),))


if __name__ == '__main__':
    unittest.main()
