"""Case-runner evidence survives source rebuilds without changing case verdicts."""
import gc
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
import warnings

import system


class CaseBinaryTests(unittest.TestCase):
    def setUp(self):
        scratch = system.ROOT / 'target/test-system-runner'
        scratch.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=scratch)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.binary = self.root / 'original'
        self.logs = self.root / 'artifacts'
        self.logs.mkdir()
        self.output = self.root / 'report.json'
        self.report = {'results': [], 'passed': False}

    def fixture(self, inventory):
        self.binary.write_text(f'''#!{sys.executable}
import pathlib, sys, time
case = sys.argv[1]
if case == '--list':
    print({inventory!r})
elif case == 'replace':
    pathlib.Path({str(self.binary)!r}).write_text('replacement executable')
elif case == 'fail':
    print('original failure')
    sys.exit(7)
elif case == 'timeout':
    time.sleep(10)
''')
        self.binary.chmod(0o755)
        return self.binary.read_bytes()

    def run_cases(self, excluded=None, timeout=5):
        system.run_cases(self.binary, self.root, excluded or {}, timeout,
                         self.output, self.report, self.logs)

    def test_failure_keeps_the_program_that_ran_even_after_source_replacement(self):
        original = self.fixture('replace fail pass')
        self.run_cases()
        self.assertEqual(self.binary.read_text(), 'replacement executable')
        self.assertFalse(self.report['passed'])
        self.assertEqual([r['status'] for r in self.report['results']], ['passed', 'failed', 'passed'])
        identity = self.report['binary']
        self.assertEqual(Path(identity['artifact']).read_bytes(), original)
        self.assertEqual(identity['sha256'], hashlib.sha256(original).hexdigest())
        self.assertIn('exit 7', (self.logs / 'fail.log').read_text())
        self.assertEqual(json.loads(self.output.read_text())['binary'], identity)

    def test_success_cleans_up_and_keeps_exclusions_distinct(self):
        original = self.fixture('replace fail pass')
        self.run_cases({'fail': 'fixture exclusion'})
        self.assertTrue(self.report['passed'])
        self.assertEqual([r['status'] for r in self.report['results']], ['passed', 'excluded', 'passed'])
        self.assertEqual(self.report['binary']['sha256'], hashlib.sha256(original).hexdigest())
        self.assertNotIn('artifact', self.report['binary'])
        self.assertFalse((self.logs / 'case-binary').exists())
        self.assertFalse((self.logs / 'fail.log').exists())

    def test_timeout_remains_failed_and_retains_the_binary(self):
        self.fixture('timeout')
        with warnings.catch_warnings(record=True) as emitted:
            warnings.simplefilter('always', ResourceWarning)
            self.run_cases(timeout=0.1)
            gc.collect()
        self.assertFalse([w for w in emitted if issubclass(w.category, ResourceWarning)])
        self.assertFalse(self.report['passed'])
        self.assertEqual(self.report['results'], [{'name': 'timeout', 'status': 'failed'}])
        self.assertTrue(Path(self.report['binary']['artifact']).is_file())
        self.assertIn('exit 124', (self.logs / 'timeout.log').read_text())

    def test_invalid_inventory_retains_evidence_before_any_case(self):
        self.fixture('duplicate duplicate')
        with self.assertRaisesRegex(ValueError, 'duplicate inventory'):
            self.run_cases()
        self.assertFalse(self.report['passed'])
        self.assertTrue(Path(self.report['binary']['artifact']).is_file())
        self.assertEqual(json.loads(self.output.read_text())['binary'], self.report['binary'])


if __name__ == '__main__':
    unittest.main()
