"""Buffer is graduated, so typed Buffer is selected by building the Rust reactor; there is no switch."""
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest

from tools import dev

ROOT = Path(__file__).resolve().parents[2]


class TypedBufferSelection(unittest.TestCase):
    def test_typed_phases_follow_the_reactor(self):
        variants = set()
        for modules in [['locks'], ['reactor']]:
            typed = 'reactor' in modules
            args = SimpleNamespace(lane='integrate', module=modules, all_modules=False,
                                   php='php', php_extension=[], jobs=2, test=None, case=None,
                                   benchmark=False, compare=False, fix_halfclose=False)
            commands = dict(dev.plan(args, ROOT/'target/reports/typed-unit'))
            self.assertTrue(all('--buffer-typed' not in command for command in commands.values()))
            for name, command in commands.items():
                if 'tests/modules/reactor/production_supported.py' in command:
                    self.assertEqual('--require-typed-buffer' in command, typed, name)
            for phase in ['buffer-typed', 'buffer-typed-layout', 'reactor-buffer-typed-compile']:
                self.assertEqual(phase in commands, typed)
            if typed:
                command=commands['buffer-typed']
                self.assertIn('--sanitize', command)
                self.assertEqual(command[command.index('--archive')+1],
                                 commands['reactor-flush'][commands['reactor-flush'].index('--archive')+1])
                compile_command=commands['reactor-buffer-typed-compile']
                self.assertEqual(compile_command[compile_command.index('--archive')+1],
                                 command[command.index('--archive')+1])
            variants.add(dev.build_variant(args))
        self.assertEqual(len(variants), 2)

    def test_the_old_switch_is_gone(self):
        for command in [[sys.executable, 'tools/dev.py', 'integrate', '--module', 'reactor', '--buffer-typed', '--plan'],
                        [sys.executable, 'tools/build_hybrid.py', '--module', 'reactor', '--buffer-typed']]:
            result=subprocess.run(command,cwd=ROOT,capture_output=True,text=True)
            self.assertEqual(result.returncode,2,result.stdout+result.stderr)
            self.assertIn('unrecognized arguments: --buffer-typed',result.stderr)
