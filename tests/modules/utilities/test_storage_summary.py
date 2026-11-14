"""Regression: every summary field is compared between engines."""
import copy
from pathlib import Path
import sys
import unittest
sys.path.insert(0, str(Path(__file__).parent))
from storage_summary import compare


class SummaryTests(unittest.TestCase):
    def fixture(self):
        # Exact summaries from frozen0656's first failed Linux production run.
        common = dict(level_cases=11, filename_cases=54, reopen_records=2,
                      caller_streams=2, thread_records=1024, thread_reopens=64)
        return [dict(engine=name, summary=dict(common)) for name in ['hybrid', 'upstream']]

    def test_every_behavior_field_is_compared(self):
        rows = self.fixture()
        compare(rows)
        for key in ['level_cases', 'filename_cases', 'reopen_records', 'caller_streams', 'thread_records', 'thread_reopens']:
            bad = copy.deepcopy(rows)
            bad[0]['summary'][key] -= 1
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'summaries differ'):
                compare(bad)

        for key, value in dict(configuration_formats=10, configuration_accepted=7,
                               configuration_rejected=3, configuration_resets=32).items():
            extended = copy.deepcopy(rows)
            for row in extended:
                row['summary'][key] = value
            compare(extended)
            extended[0]['summary'][key] -= 1
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'summaries differ'):
                compare(extended)

    def test_other_profiles_retain_full_summary_equality(self):
        with self.assertRaisesRegex(ValueError, 'summaries differ'):
            compare([dict(summary=dict(records=3)), dict(summary=dict(records=2))])


if __name__ == '__main__':
    unittest.main()
