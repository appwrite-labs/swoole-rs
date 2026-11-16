"""The API fuzzer masks process IDs, and nothing else of a result, wherever r() prints one (#4492)."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('api_fuzz', Path(__file__).resolve().parents[2] / 'tests/differential/fuzz.py')
fuzz = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fuzz)


def wait_pid(pid, code=0, signal=0):
    return f"sys.waitPid = ['pid'=>{pid},'code'=>{code},'signal'=>{signal}]\n"


class PidMask(unittest.TestCase):
    def test_a_wait_pid_array_differs_in_its_pid_alone(self):
        self.assertEqual(fuzz.mask(wait_pid(22131)), fuzz.mask(wait_pid(22134)))

    def test_its_code_and_signal_still_differ(self):
        self.assertNotEqual(fuzz.mask(wait_pid(1, code=0)), fuzz.mask(wait_pid(1, code=1)))
        self.assertNotEqual(fuzz.mask(wait_pid(1, signal=0)), fuzz.mask(wait_pid(1, signal=9)))

    def test_false_still_differs_from_an_array(self):
        self.assertNotEqual(fuzz.mask('sys.waitPid = false\n'), fuzz.mask(wait_pid(22131)))

    def test_the_other_spellings_stay_masked(self):
        for one, other in [('pid 1', 'pid 2'), ('pid=1', 'pid=2'), ('process #1', 'process #2'), ('PID: 1', 'PID: 2')]:
            self.assertEqual(fuzz.mask(one), fuzz.mask(other))


if __name__ == '__main__':
    unittest.main()
