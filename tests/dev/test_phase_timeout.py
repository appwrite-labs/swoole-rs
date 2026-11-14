"""dev.py phase timeouts kill the phase's whole process group; a crashing phase fails instead of hanging."""
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import dev  # noqa: E402


class PhaseTimeout(unittest.TestCase):
    def test_timeout_kills_the_process_group(self):
        with tempfile.TemporaryDirectory() as directory:
            pid_file = Path(directory) / 'grandchild'
            # The phase backgrounds a long-lived child (like a test server), then hangs itself.
            command = ['sh', '-c', f'sleep 60 & echo $! > {pid_file}; sleep 60']
            started = time.monotonic()
            result = dev.execute(command, timeout=1)
            self.assertIsNone(result.returncode)
            self.assertLess(time.monotonic() - started, 10)
            grandchild = int(pid_file.read_text())
            time.sleep(0.2)
            # Killed but unreaped (a zombie) counts as gone: not every container's init reaps orphans.
            stat = Path(f'/proc/{grandchild}/stat')
            if not (stat.exists() and stat.read_text().rpartition(')')[2].split()[0] == 'Z'):
                with self.assertRaises(ProcessLookupError):
                    os.kill(grandchild, 0)

    def run_main(self, planned, *extra, execute=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            patches = [patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned),
                       patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', *extra])]
            if execute:
                patches.append(patch.object(dev, 'execute', side_effect=execute))
            for p in patches:
                p.start()
            try:
                code = dev.main()
            finally:
                for p in reversed(patches):
                    p.stop()
            report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
        return code, {row['phase']: row for row in report['phases']}

    def test_a_timed_out_phase_fails_and_the_run_continues(self):
        code, rows = self.run_main([('slow', ['sleep', '30']), ('quick', ['true'])], '--phase-timeout', '1')
        self.assertNotEqual(code, 0)
        self.assertTrue(rows['slow']['failed'])
        self.assertTrue(rows['slow']['timed_out'])
        self.assertFalse(rows['quick']['failed'])
        # A failed phase keeps the end of its log in loop.json; a passing one doesn't.
        self.assertIn('timed out after 1s', rows['slow']['log_tail'][-1])
        self.assertNotIn('log_tail', rows['quick'])

    def test_a_failed_phase_is_annotated(self):
        with patch.object(dev, 'annotate') as annotate:
            code, rows = self.run_main([('broken', ['false']), ('fine', ['true'])])
        self.assertNotEqual(code, 0)
        annotate.assert_called_once_with('error', 'broken failed', f"exit 1, {rows['broken']['log']}")

    def test_a_crashing_phase_fails_instead_of_hanging(self):
        def execute(command, **options):
            raise RuntimeError('boom')
        started = time.monotonic()
        code, rows = self.run_main([('broken', ['x'])], execute=execute)
        self.assertLess(time.monotonic() - started, 10)
        self.assertNotEqual(code, 0)
        self.assertTrue(rows['broken']['failed'])
        self.assertIn('RuntimeError: boom', rows['broken']['outcome_error'])


if __name__ == '__main__':
    unittest.main()
