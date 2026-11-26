"""autobahn.py's verdict: a case fails by doing worse than on the control engine or by leaving the baseline."""
from pathlib import Path
import tempfile
import unittest

from autobahn import against_baseline, regressions


def case(behavior, close='OK'):
    return {'behavior': behavior, 'behaviorClose': close}


class Verdict(unittest.TestCase):
    def test_worse_than_the_control_in_either_field(self):
        upstream = {'1.1.1': case('OK'), '2.1': case('NON-STRICT'), '3.1': case('FAILED'),
                    '4.1': case('OK'), '5.1': case('INFORMATIONAL'), '6.1': case('OK'), '7.1': case('OK')}
        cargo = {'1.1.1': case('OK'), '2.1': case('FAILED'), '3.1': case('FAILED'),
                 '4.1': case('OK', 'WRONG CODE'), '5.1': case('INFORMATIONAL'), '6.1': case('NON-STRICT')}
        self.assertEqual(regressions(upstream, cargo), ['2.1', '4.1', '6.1', '7.1'],
                         'equal or failing on both passes; worse, or missing, fails')
        self.assertEqual(regressions(cargo, upstream), [], 'doing better than the control is fine')

    def test_baseline(self):
        with tempfile.TemporaryDirectory() as work:
            path = Path(work) / 'autobahn.baseline'
            path.write_text('# header\n1.1.1\n2.1  # note\n\n3.1\n')
            self.assertEqual(against_baseline(path, ['1.1.1', '4.1'], ['1.1.1', '2.1', '4.1']), (['2.1'], ['4.1']),
                             'a listed case that ran and failed is lost; one that did not run is not; new passes list')


if __name__ == '__main__':
    unittest.main()
