"""The new real-entry runner preserves both first engine outcomes on failure."""
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
spec=importlib.util.spec_from_file_location('phphelpers_entrypoint',Path(__file__).with_name('entrypoint.py'))
entrypoint=importlib.util.module_from_spec(spec);spec.loader.exec_module(entrypoint)


class EntrypointOutcomes(unittest.TestCase):
    def test_initial_native_failure_keeps_selected_attempt(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);extension=root/'engine.so';extension.write_bytes(b'fixture');sdk=root/'sdk.json';sdk.write_text('{}')
            helper=root/'helper.so';helper.write_bytes(b'helper');output=root/'report.json';engines=[]
            for name,bits in [('native',0),('hybrid',16777216)]:
                identity=dict(php='fixture',name=name,extension=str(extension),sha256=entrypoint.sha(extension),rust_feature_bits=bits)
                engines.append(SimpleNamespace(name=name,metadata=lambda identity=identity:identity,command=lambda *args,name=name:[name,*args]))
            results=[SimpleNamespace(returncode=7,stdout='native first failure',stderr='native diagnostic'),
                     SimpleNamespace(returncode=0,stdout='{"fixture":true}',stderr='')]
            def build(*args, **kwargs):
                args[2]['helper_compilation']={'sdk_php':'fixture'}
                return helper
            argv=['entrypoint.py','--sdk-report',str(sdk),'--engine','native=fixture','--engine','hybrid=fixture',
                  '--build-manifest','fixture','--archive','fixture','--source-ref','fixture','--output',str(output)]
            with patch.object(sys,'argv',argv),patch.object(entrypoint,'parse_engines',return_value=engines), \
                 patch.object(entrypoint,'attest',return_value={}),patch.object(entrypoint,'build_helper',side_effect=build), \
                 patch.object(entrypoint,'run_isolated',side_effect=results) as runs,contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(entrypoint.main(),1)
            report=json.loads(output.read_text());self.assertFalse(report['passed']);self.assertEqual(runs.call_count,2)
            self.assertEqual(report['results'][0]['exit'],7);self.assertEqual(report['results'][0]['stdout'],'native first failure')
            self.assertTrue(report['results'][1]['passed']);self.assertEqual(report['child_results'],[])


if __name__=='__main__':unittest.main()
