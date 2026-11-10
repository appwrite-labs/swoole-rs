"""Appwrite e2e reports gate on Paratest's totals, which survive its lossy merge."""
from pathlib import Path
import tempfile
import unittest

from tools.appwrite_e2e import compare, load


def junit(tests, failing=(), kept=()):
    """A Paratest-shaped report: complete totals, only the `kept` testcases."""
    cases = ''.join(f'<testcase class="T" name="{name}">{"<failure/>" if name in failing else ""}</testcase>'
                    for name in kept)
    return (f'<testsuites><testsuite name="" tests="{tests}" failures="{len(failing)}" errors="0" skipped="1">'
            f'<testsuite name="T" tests="{len(kept)}">{cases}</testsuite></testsuite></testsuites>')


class CompareTest(unittest.TestCase):
    def compare(self, upstream, cargo):
        directory = Path(tempfile.mkdtemp())
        for variant, services in [('upstream', upstream), ('cargo', cargo)]:
            for service, xml in services.items():
                path = directory / f'junit-{variant}-{service}' / 'junit.xml'
                path.parent.mkdir()
                path.write_text(xml)
        return compare(load(directory))

    def test_matching_totals_pass_even_when_testcases_were_dropped(self):
        rows, regressions, flaky = self.compare({'Account': junit(183, kept=['a', 'b'])},
                                                {'Account': junit(183, kept=['c'])})
        self.assertEqual((regressions, flaky), ([], []))
        self.assertEqual(rows[0][1][:3], (183, 0, 1))

    def test_more_failures_fewer_tests_or_no_report_regress(self):
        _, regressions, _ = self.compare(
            {'Account': junit(10), 'Teams': junit(10), 'Users': junit(10)},
            {'Account': junit(10, failing=['b'], kept=['a', 'b']), 'Teams': junit(9)})
        self.assertEqual(regressions, ['Account: 10 tests, 1 failing (upstream 10, 0)\n  - T::b',
                                       'Teams: 9 tests, 0 failing (upstream 10, 0)',
                                       'Users: no cargo report'])

    def test_upstream_failures_are_listed_not_gated(self):
        _, regressions, flaky = self.compare({'Account': junit(10, failing=['a'], kept=['a'])},
                                             {'Account': junit(10, failing=['a'], kept=['a'])})
        self.assertEqual((regressions, flaky), ([], ['Account: 1 failing on upstream\n  - T::a']))


if __name__ == '__main__':
    unittest.main()
