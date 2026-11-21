"""dev.py --shard: slices cover the full plan, builds run everywhere, readers stay with producers."""
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import dev  # noqa: E402


def plan(*args):
    result = subprocess.run([sys.executable, 'tools/dev.py', 'integrate', '--all-modules', *args, '--plan'],
                            cwd=ROOT, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    return {row['phase']: row['command'] for row in json.loads(result.stdout)}


class PlanShards(unittest.TestCase):
    def test_slices_partition_the_plan(self):
        full = plan()
        for count in [2, 3]:
            slices = [plan('--shard', f'{index}/{count}') for index in range(1, count + 1)]
            self.assertEqual(set().union(*slices), set(full))
            for barrier in dev.BARRIER_PHASES & set(full):
                self.assertTrue(all(barrier in s for s in slices), barrier)
            # Apart from barriers and what they read, every phase runs in exactly one slice.
            everywhere = set.intersection(*(set(s) for s in slices))
            for name in set(full) - everywhere:
                self.assertEqual(sum(name in s for s in slices), 1, name)
            for s in slices:
                for name, command in s.items():
                    for arg in command:
                        if arg.endswith(('.json', '.xml')) and '/reports/dev/' in arg:
                            producer = Path(arg).stem
                            if producer in full and producer != name:
                                self.assertIn(producer, s, f'{name} reads {producer}')

    def test_shard_needs_the_full_integrate_plan(self):
        result = subprocess.run([sys.executable, 'tools/dev.py', 'integrate', '--all-modules', '--shard', '1/2',
                                 '--phase', 'build', '--plan'], cwd=ROOT, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('--shard', result.stderr)

    def test_weights_name_only_merge_tier_phases(self):
        # A renamed phase silently loses its weight and falls back to DEFAULT_WEIGHT.
        argv = ['dev.py', 'integrate', '--all-modules', '--php-sockets', '--external-network', '--plan']
        with mock.patch.object(sys, 'argv', argv), mock.patch.object(sys, 'platform', 'linux'), \
                redirect_stdout(io.StringIO()) as out:
            self.assertEqual(dev.main(), 0)
        planned = {row['phase'] for row in json.loads(out.getvalue())}
        self.assertEqual(set(json.loads(dev.PHASE_WEIGHTS.read_text())) - planned, set())


if __name__ == '__main__':
    unittest.main()
