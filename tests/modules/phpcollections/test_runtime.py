"""The pair runner must attest real identities before dispatching PHP fixtures."""
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from contextlib import redirect_stdout

sys.path.insert(0,str(Path(__file__).resolve().parent))
SPEC=importlib.util.spec_from_file_location('collection_runtime',Path(__file__).with_name('php_runtime.py'))
runtime=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(runtime)


class AttestedPair(unittest.TestCase):
    def run_pair(self, bits=None, attestation_error=False, warning=False, mismatch=False, checks=330, profile=None):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder)
            for name in ['native.so','selected.so','build.json']:
                (root/name).write_bytes(name.encode())
            output=root/'report.json'
            argv=['runner','--engine','native='+str(root/'native.so'),'--engine','selected='+str(root/'selected.so'),
                  '--build-manifest',str(root/'build.json'),'--output',str(output)]
            if profile: argv += ['--profile', profile]
            fixtures=[]
            def run(command,**kwargs):
                selected=any('engine-1.so' in str(item) for item in command)
                if '-r' in command:
                    text=json.dumps(dict(php='fixture',rust_feature_bits=(runtime.BIT if selected else 0) if bits is None else bits))
                    return SimpleNamespace(returncode=0,stdout=text,stderr='startup warning' if warning else '')
                fixtures.append(command)
                return SimpleNamespace(returncode=0,stdout=json.dumps({'checks':checks,'marker':1 if mismatch and selected else 0}),stderr='')
            def attest(*args):
                if attestation_error:raise ValueError('wrong frozen source')
                self.assertEqual(args[-1],runtime.BIT)
                self.assertTrue(Path(args[1]['extension']).is_file())
                return dict(manifest_sha256=runtime.sha(root/'build.json'))
            with patch.object(sys,'argv',argv),patch.object(runtime,'require_build_space'),patch.object(runtime,'run_isolated',side_effect=run),patch.object(runtime,'attest',side_effect=attest) as attested,redirect_stdout(io.StringIO()):
                status=runtime.main()
            return status,json.loads(output.read_text()),len(fixtures),attested.call_count

    def test_pair_qualifies_only_after_build_attestation(self):
        status,report,fixtures,attestations=self.run_pair()
        self.assertEqual((status,fixtures,attestations),(0,2,1))
        self.assertTrue(report['passed'])

    def test_matching_truncated_inventories_cannot_pass(self):
        status,report,fixtures,attestations=self.run_pair(checks=0)
        self.assertEqual((status,fixtures,attestations),(1,2,1))
        self.assertFalse(report['passed'])
        self.assertTrue(all(row['summary']['checks']==0 and not row['passed'] for row in report['results']))
        self.assertTrue(all('inventory_error' in row for row in report['results']))

    def test_binary_column_profile_is_separate_and_exact(self):
        status,report,fixtures,attestations=self.run_pair(checks=16,profile='column-bytes')
        self.assertEqual((status,fixtures,attestations),(0,2,1))
        self.assertEqual(report['profile'],'column-bytes')
        self.assertEqual(report['expected_checks'],16)
        status,report,_,_=self.run_pair(checks=324,profile='column-bytes')
        self.assertEqual(status,1)
        self.assertFalse(report['passed'])

    def test_missing_actual_module_bit_refuses_before_fixture(self):
        status,report,fixtures,attestations=self.run_pair(bits=0)
        self.assertEqual((status,fixtures,attestations),(1,0,0))
        self.assertIn('M27-bit',report['error'])

    def test_stale_source_refuses_before_fixture(self):
        status,report,fixtures,attestations=self.run_pair(attestation_error=True)
        self.assertEqual((status,fixtures,attestations),(1,0,1))
        self.assertIn('wrong frozen source',report['error'])

    def test_metadata_warning_is_preserved_and_not_accepted(self):
        status,report,fixtures,attestations=self.run_pair(warning=True)
        self.assertEqual((status,fixtures,attestations),(1,0,0))
        self.assertEqual(report['metadata_commands'][0]['stderr'],'startup warning')

    def test_different_summaries_fail_with_both_outputs(self):
        status,report,fixtures,attestations=self.run_pair(mismatch=True)
        self.assertEqual((status,fixtures,attestations),(1,2,1))
        self.assertEqual(len(report['results']),2)
        self.assertIn('summaries differ',report['error'])

if __name__=='__main__':unittest.main()
