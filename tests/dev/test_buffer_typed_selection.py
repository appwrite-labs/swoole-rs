"""Every build has the one typed Buffer queue; there is no switch."""
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest

from tools import dev

ROOT = Path(__file__).resolve().parents[2]


class TypedBufferSelection(unittest.TestCase):
    def test_typed_phases_always_run(self):
        args = SimpleNamespace(lane='integrate', module=['reactor'], all_modules=False,
                               php='php', php_extension=[], jobs=2, test=None, case=None,
                               benchmark=False, compare=False, fix_halfclose=False)
        commands = dict(dev.plan(args, ROOT/'target/reports/typed-unit'))
        self.assertTrue(all('--buffer-typed' not in command for command in commands.values()))
        command = commands['buffer-typed']
        self.assertIn('--sanitize', command)
        archive = command[command.index('--archive')+1]
        self.assertTrue(archive.endswith('sdk/lib/libswoole_rs_ffi.a'), archive)

    def test_the_old_switch_is_gone(self):
        for command in [[sys.executable, 'tools/dev.py', 'integrate', '--module', 'reactor', '--buffer-typed', '--plan'],
                        [sys.executable, 'tools/build_ext.py', '--buffer-typed']]:
            result=subprocess.run(command,cwd=ROOT,capture_output=True,text=True)
            self.assertEqual(result.returncode,2,result.stdout+result.stderr)
            self.assertIn('unrecognized arguments: --buffer-typed',result.stderr)
