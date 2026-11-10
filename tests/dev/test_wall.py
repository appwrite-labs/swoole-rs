"""wall.py keeps a counter one repetition lost and drops one no repetition counted (#2526)."""
import unittest
from benchmarks.wall import summary


def row(task_clock):
    return {'requests_per_second': 1e5, 'p50_ms': 0.25, 'p99_ms': 0.5, 'per_request': {'task-clock': task_clock}}


class Summary(unittest.TestCase):
    def test_one_uncounted_repetition_keeps_the_median_of_the_rest(self):
        result = summary([row(v) for v in (0.009, None, 0.008, 0.010, 0.012)])
        self.assertEqual(result['task-clock']['median'], 0.0095)
        self.assertEqual(result['task-clock']['samples'], 4)
        self.assertEqual(result['p50_ms']['samples'], 5)

    def test_a_counter_no_repetition_counted_is_absent(self):
        self.assertNotIn('task-clock', summary([row(None)] * 5))
        self.assertNotIn('cycles', summary([row(0.009)] * 5))


if __name__ == '__main__':
    unittest.main()
