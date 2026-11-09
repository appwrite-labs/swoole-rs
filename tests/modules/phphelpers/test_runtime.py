"""A new supplemental fixture preserves both engines' first failure outputs."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('phphelpers_runtime',Path(__file__).with_name('runtime.py'))
runtime=importlib.util.module_from_spec(spec);spec.loader.exec_module(runtime)


class RuntimeOutcomes(unittest.TestCase):
    def test_native_failure_does_not_hide_selected_attempt(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);extension=root/'engine.so';extension.write_bytes(b'fixture')
            output=root/'report.json'
            engines=[]
            for name,bits in [('native',0),('hybrid',16777216)]:
                identity={'name':name,'extension':str(extension),'sha256':runtime.sha(extension),'rust_feature_bits':bits}
                engines.append(SimpleNamespace(name=name,metadata=lambda identity=identity:identity,command=lambda path,name=name:[name,path]))
            results=[SimpleNamespace(returncode=7,stdout='original failure',stderr='native diagnostic'),
                     SimpleNamespace(returncode=0,stdout='{"fixture":true}',stderr='')]
            argv=['runtime.py','--engine','native=fixture','--engine','hybrid=fixture','--build-manifest','fixture',
                  '--output',str(output)]
            with patch.object(sys,'argv',argv),patch.object(runtime,'parse_engines',return_value=engines), \
                 patch.object(runtime,'attest',return_value={}),patch.object(runtime,'unchanged'),patch.object(runtime,'run_isolated',side_effect=results) as runs, \
                 contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(runtime.main(),1)
            report=json.loads(output.read_text());self.assertFalse(report['passed'])
            self.assertEqual(runs.call_count,2);self.assertEqual(len(report['results']),2)
            self.assertEqual(report['results'][0]['exit'],7)
            self.assertEqual(report['results'][0]['stdout'],'original failure')
            self.assertTrue(report['results'][1]['passed'])


if __name__=='__main__':unittest.main()
