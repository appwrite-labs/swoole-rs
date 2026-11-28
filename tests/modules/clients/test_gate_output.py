"""Command evidence survives binary diagnostics; success remains strict UTF-8."""
import base64
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import gate

class CommandEvidence(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        previous=(gate.LOG,gate.SERIAL)
        self.addCleanup(lambda:setattr(gate,'LOG',previous[0]))
        self.addCleanup(lambda:setattr(gate,'SERIAL',previous[1]))
        gate.LOG=Path(self.directory.name);gate.SERIAL=0
    def record(self):
        return json.loads((gate.LOG/'001.json').read_text())
    def test_nonzero_binary_failure_keeps_status_and_original_bytes(self):
        with self.assertRaises(subprocess.CalledProcessError) as failure:
            gate.run([sys.executable,'-c','import os;os.write(1,bytes([255]));os.write(2,bytes([254]));raise SystemExit(7)'])
        self.assertEqual(failure.exception.returncode,7)
        record=self.record()
        self.assertEqual(record['returncode'],7)
        self.assertEqual(base64.b64decode(record['stdout_raw_base64']),bytes([255]))
        self.assertEqual(base64.b64decode(record['stderr_raw_base64']),bytes([254]))
    def test_success_source_utf8_is_unchanged(self):
        source='source §'
        self.assertEqual(gate.run([sys.executable,'-c','print("source §",end="")']),source)
        self.assertEqual(self.record()['stdout'],source)
        self.assertNotIn('stdout_raw_base64',self.record())
    def test_zero_exit_with_binary_output_still_fails_strict_decode(self):
        with self.assertRaises(UnicodeDecodeError):
            gate.run([sys.executable,'-c','import os;os.write(1,bytes([255]))'])
        record=self.record()
        self.assertEqual(record['returncode'],0)
        self.assertEqual(base64.b64decode(record['stdout_raw_base64']),bytes([255]))

if __name__=='__main__':unittest.main()
