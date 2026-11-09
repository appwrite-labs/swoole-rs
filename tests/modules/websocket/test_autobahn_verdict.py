"""autobahn.py's verdict: a case fails only by doing worse than on the control engine."""
import unittest

from autobahn import regressions


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


if __name__ == '__main__':
    unittest.main()
