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

    def test_merge_reads_the_newest_finished_run(self):
        self.assertEqual(badges.merge('abcdef0123:success'),
                         badges.endpoint('merge tier', 'passing @ abcdef0', 'brightgreen'))
        self.assertEqual(badges.merge('abcdef0123:failure'),
                         badges.endpoint('merge tier', 'failing @ abcdef0', 'red'))

    def test_unsafe_table_lists_crates_most_first_and_marks_forbid(self):
        table = badges.unsafe_table({'zend': 1200, 'clients': 0, 'server': 3400},
                                    {'zend': True, 'clients': False, 'server': True}, 'abcdef0123')
        self.assertLess(table.index('`server`'), table.index('`zend`'))
        self.assertLess(table.index('`zend`'), table.index('`clients`'))
        self.assertIn('| `server` | 3,400 |  |', table)
        self.assertIn('| `clients` | 0 | ✅ |', table)
        self.assertIn('`abcdef0`', table)
        self.assertIn('Total 4,600.', table)


if __name__ == '__main__':
    unittest.main()
