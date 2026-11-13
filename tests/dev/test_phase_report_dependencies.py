"""Reject leaf selections whose report inputs cannot exist in the new run."""
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PhaseReports(unittest.TestCase):
    def test_report_consumers_require_their_parent_phase(self):
        for module, child, parent in [
            ('locks','base64-consumers-upstream','reference-identity')]:  # base64 is graduated, so always selected
            with self.subTest(child=child):
                command=[sys.executable,'tools/dev.py','integrate','--module',module,'--phase',child,'--plan']
                failed=subprocess.run(command,cwd=ROOT,capture_output=True,text=True)
                self.assertEqual(failed.returncode,2,failed.stdout+failed.stderr)
                self.assertIn('requires report-producing phases',failed.stderr)
                self.assertIn(parent,failed.stderr)
                result=subprocess.run(command+['--phase',parent],cwd=ROOT,capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stderr)
                phases=[row['phase'] for row in json.loads(result.stdout)]
                self.assertEqual(set(phases),{parent,child})
                self.assertLess(phases.index(parent),phases.index(child))

    def test_with_prerequisites_adds_reports_and_builds(self):
        command=[sys.executable,'tools/dev.py','integrate','--module','locks','--phase','base64-consumers-upstream',
                 '--with-prerequisites','--plan']
        result=subprocess.run(command,cwd=ROOT,capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stderr)
        phases=[row['phase'] for row in json.loads(result.stdout)]
        # The report it reads, then the upstream reference build behind both, in plan order.
        self.assertIn('reference-identity',phases)
        self.assertNotIn('build',phases)
        self.assertLess(phases.index('reference-build'),phases.index('base64-consumers-upstream'))
        self.assertLess(phases.index('reference-identity'),phases.index('base64-consumers-upstream'))
        self.assertNotIn('base64-consumers-hybrid',phases)

    def test_independent_selection_stays_independent(self):
        result=subprocess.run([sys.executable,'tools/dev.py','integrate','--module', 'locks',
            '--phase','string-originals','--plan'],cwd=ROOT,capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual([r['phase'] for r in json.loads(result.stdout)],['string-originals'])


class ExternalNetwork(unittest.TestCase):
    def test_flag_reaches_local_phpt(self):
        result=subprocess.run([sys.executable,'tools/dev.py','candidate','--module', 'locks','--external-network',
            '--phase','phpt','--plan'],cwd=ROOT,capture_output=True,text=True)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIn('--external-network',json.loads(result.stdout)[0]['command'])
