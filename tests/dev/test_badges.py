"""tools/badges.py counts passing PHPTs and speed."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import badges  # noqa: E402


class Badges(unittest.TestCase):
    def test_phpt_counts_cargo_passes_over_every_pinned_test(self):
        rows = [{'test': 'a', 'engine': 'upstream', 'status': 'passed'}, {'test': 'a', 'engine': 'cargo', 'status': 'passed'},
                {'test': 'b', 'engine': 'upstream', 'status': 'passed'}, {'test': 'b', 'engine': 'cargo', 'status': 'failed'},
                {'test': 'c', 'engine': 'upstream', 'status': 'skipped'}, {'test': 'c', 'engine': 'cargo', 'status': 'skipped'},
                {'test': 'd', 'engine': 'upstream', 'status': 'passed'}, {'test': 'd', 'engine': 'cargo', 'status': 'failed', 'rerun': 'passed'}]
        self.assertEqual(badges.phpt_passing(rows), (2, 4))
        # Another job's gate (a build variant) passes b; c can't apply here.
        rows.append({'test': 'b', 'engine': 'cargo', 'status': 'passed'})
        self.assertEqual(badges.phpt_passing(rows, {'c': 'darwin only'}), (3, 3))

    def test_speed_counts_workloads_faster_than_upstream_and_names_the_slowest(self):
        result = {'requests': 10, 'instructions_per_request': {
            'plaintext': {'cargo': 103.0, 'upstream': 100.0}, 'timer': {'cargo': 80.0, 'upstream': 100.0},
            'websocket': {'cargo': 120.0, 'upstream': 100.0}}}
        badge, table = badges.speed(result, {'plaintext': 1.05, 'websocket': 1.1}, 'abcdef0123')
        self.assertEqual(badge['message'], '1/3 faster · slowest websocket 1.20×')
        self.assertEqual(badge['color'], 'orange')
        self.assertIn('| websocket | 120 | 100 | 1.2000× | 1.1× | ❌ |', table)
        self.assertIn('| timer | 80 | 100 | 0.8000× |  | ✅ |', table)
        self.assertIn('`abcdef0`', table)


if __name__ == '__main__':
    unittest.main()
