"""corpus/upstream/services.py: names services.yml's services, and an unknown family gets them all."""
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'corpus/upstream'))
import services  # noqa: E402


class PhptServices(unittest.TestCase):
    def test_names_match_the_compose_file(self):
        declared = re.findall(r'^  ([a-z0-9-]+):$', (ROOT / 'corpus/upstream/services.yml').read_text(), re.M)
        self.assertEqual(sorted(declared), sorted(services.SERVICES))
        for family, needed in services.NEEDS.items():
            self.assertLessEqual(set(needed), set(services.SERVICES), family)

    def test_selection(self):
        self.assertEqual(services.services([]), services.SERVICES)
        self.assertEqual(services.services(['swoole_timer']), [])
        self.assertEqual(services.services(['swoole_timer', 'swoole_pdo_pgsql', 'swoole_server']),
                         ['pgsql', 'mysql', 'redis'])
        self.assertEqual(services.services(['swoole_timer', 'swoole_not_in_the_map']), services.SERVICES)


if __name__ == '__main__':
    unittest.main()
