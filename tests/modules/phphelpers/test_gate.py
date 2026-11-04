"""A fixture failure fails the gate and keeps its command."""
import contextlib
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
sys.path.insert(0,str(Path(__file__).parent))
import gate


class GateOutcomes(unittest.TestCase):
    def test_compile_failure_is_failed_with_its_command_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);archive=root/'fixture.a';archive.write_bytes(b'fixture');output=root/'report.json'
            common='static zend_always_inline zval *sw_zend_symtable_str_add() {}\nnamespace function {}\nCallable::Callable(int x) {}\nbool call() {}\nVariable call() {}'
            hybrid=root/'hybrid';(hybrid/'ext-src').mkdir(parents=True)
            (hybrid/'ext-src/php_swoole_cxx.cc').write_text('#ifndef SWOOLE_RS_PHPHELPERS\n'+common.replace('\nnamespace function','\n#endif\nnamespace function',1))
            attempts=[]
            def run(command,**kwargs):
                attempts.append(command)
                if len(attempts)==1:return SimpleNamespace(returncode=7,stdout='',stderr='late compile failure')
                if '-o' in command:Path(command[command.index('-o')+1]).write_bytes(b'fixture binary')
                return SimpleNamespace(returncode=0,stdout='{"aliases":151}',stderr='')
            argv=['gate.py','--source',str(root),'--archive',str(archive),'--output',str(output)]
            with patch.object(sys,'argv',argv),patch.object(gate,'require_build_space'),patch.object(gate,'hybrid_tree',return_value=(hybrid,[])), \
                 patch.object(gate.subprocess,'check_output',side_effect=lambda args,**kw:common), \
                 patch.object(gate,'run_isolated',side_effect=run),contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(gate.main(),1)
            report=json.loads(output.read_text())
            self.assertFalse(report['passed']);self.assertEqual(len(report['commands']),1)
            self.assertEqual(report['commands'][-1]['returncode'],7)
            self.assertIn('native-effect fixture compilation failed',report['error'])


if __name__=='__main__':unittest.main()
