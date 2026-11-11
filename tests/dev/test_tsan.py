"""tools/tsan.py's LIMIT counts every report outside upstream's runs, exit_guard's included."""
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import tsan  # noqa: E402


class OutsideUpstream(unittest.TestCase):
    def test_upstream_runs_are_left_out(self):
        counts = {'upstream': 5, 'upstream-guard': 3, 'cargo': 2, 'cargo-guard': 1}
        self.assertEqual(tsan.outside_upstream(counts), 3)


if __name__ == '__main__':
    unittest.main()
