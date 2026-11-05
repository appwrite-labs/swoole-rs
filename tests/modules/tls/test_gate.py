"""Receipt/precondition tests only; synthetic bytes are never runtime evidence."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('tls_gate', Path(__file__).with_name('gate.py'))
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class ControlReceipt(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.tree = self.root/'native'; (self.tree/'lib').mkdir(parents=True)
        self.config = self.root/'selected.h'; self.config.write_bytes(b'synthetic config')
        (self.tree/'config.h').write_bytes(self.config.read_bytes())
        self.library = self.tree/'lib'/('libswoole.dylib' if sys.platform == 'darwin' else 'libswoole.so')
        self.library.write_bytes(b'synthetic library, never executed')
        self.report = self.root/'report.json'
        self.row = dict(name='upstream', configure=dict(returncode=0,command=['cmake','-S',str(self.tree)]),
            build=dict(returncode=0), library_sha256=gate.sha(self.library), compiled_config_sha256=gate.sha(self.config))
        self.data = dict(upstream_pin=gate.PIN, engines=[self.row], passed=False,
                         execution_status='failed', error='retained unrelated selected-engine failure')

    def read(self):
        self.report.write_text(json.dumps(self.data))
        return gate.control_from_report(self.report, self.config)

    def test_successfully_built_control_preserves_parent_failure(self):
        result = self.read()
        self.assertEqual(result['library'], self.library.resolve())
        self.assertFalse(result['parent_passed'])
        self.assertEqual(result['parent_error'], self.data['error'])

    def test_failed_native_build_rejected(self):
        self.row['build']['returncode'] = 1
        with self.assertRaisesRegex(gate.PreconditionError, 'build failed'): self.read()

    def test_config_and_library_mutation_rejected(self):
        self.config.write_bytes(b'other config')
        with self.assertRaisesRegex(gate.PreconditionError, 'configuration differ'): self.read()
        self.config.write_bytes((self.tree/'config.h').read_bytes())
        self.library.write_bytes(b'changed library')
        with self.assertRaisesRegex(gate.PreconditionError, 'attested build'): self.read()

    def test_ambiguous_original_rejected(self):
        self.data['engines'].append(dict(self.row))
        with self.assertRaisesRegex(gate.PreconditionError, 'ambiguous'): self.read()

    def test_binary_only_cache_rejected(self):
        self.data = dict(source_commit=gate.PIN, status='complete', cache=str(self.root), inputs=dict(hybrid=None))
        with self.assertRaisesRegex(gate.PreconditionError, 'binary-only'): self.read()

    def test_native_core_attestation(self):
        self.data = dict(source_commit=gate.PIN, status='complete', cache=str(self.root),
            native_library=dict(resolved_path=str(self.library),sha256=gate.sha(self.library)),
            inputs=dict(config_sha256=gate.sha(self.config), hybrid=None))
        (self.root/'src').symlink_to(self.tree, target_is_directory=True)
        self.assertEqual(self.read()['library_sha256'], gate.sha(self.library))
        self.data['inputs']['hybrid'] = {'flags': [], 'files': {}}
        with self.assertRaisesRegex(gate.PreconditionError, 'original full-runtime'): self.read()


    def nested(self):
        (self.root/'src').symlink_to(self.tree,target_is_directory=True)
        child=self.root/'child.json'
        child.write_text(json.dumps(dict(source_commit=gate.PIN,status='complete',passed=False,
            cache=str(self.root),native_library=dict(resolved_path=str(self.library),sha256=gate.sha(self.library)),
            inputs=dict(config_sha256=gate.sha(self.config),hybrid=None))))
        self.data=dict(passed=False,status='failed',error='retained selected downstream failure',
            builds=dict(native=dict(report=str(child),sha256=gate.sha(child),exit=1,
                selected_cases_passed=True,full_inventory_passed=False,excluded_units=['unsupported'],case_count=33)))
        return child

    def test_outer_attests_child_and_retains_partial_inventory(self):
        child=self.nested();result=self.read()
        self.assertEqual(result['library'],self.library.resolve())
        self.assertEqual([x['path'] for x in result['report_chain']],[str(self.report.resolve()),str(child.resolve())])
        self.assertFalse(result['parent_passed'])
        self.assertFalse(result['outer_native']['full_inventory_passed'])
        self.assertEqual(result['outer_native']['exit'],1)
        self.assertFalse(result['child_parent']['parent_passed'])
        self.assertEqual(result['outer_native']['excluded_units'],['unsupported'])
        self.assertEqual(result['parent_error'],'retained selected downstream failure')

    def test_outer_child_hash_mutation_rejected(self):
        child=self.nested();child.write_text(child.read_text()+' ')
        with self.assertRaisesRegex(gate.PreconditionError,'path/hash differs'): self.read()

    def test_outer_still_requires_exact_config(self):
        self.nested();self.config.write_bytes(b'other configuration')
        with self.assertRaisesRegex(gate.PreconditionError,'configuration differ'): self.read()

    def test_outer_cannot_attest_another_outer(self):
        child=self.nested();child.write_text(json.dumps(self.data))
        self.data['builds']['native']['sha256']=gate.sha(child)
        with self.assertRaisesRegex(gate.PreconditionError,'one direct native-core child'): self.read()


class OrdinaryAllocationControl(unittest.TestCase):
    def test_sanitizer_references_reject_incompatible_ordinary_runtime(self):
        output='                 U __asan_init\n                 U __asan_report_load8\n                 U __ubsan_handle_type_mismatch_v1\n0000 T operator_new\n'
        self.assertEqual(gate.sanitizer_symbols(output),
            ['__asan_init','__asan_report_load8','__ubsan_handle_type_mismatch_v1'])
        self.assertEqual(gate.sanitizer_symbols('                 U malloc\n0000 T _Znwm\n'), [])


class CryptoOperationGuard(unittest.TestCase):
    def test_dh_operation_requires_allocator_profile(self):
        command=[sys.executable,str(Path(__file__).with_name('differential.py')),
            '--source','/unused','--reference','/unused','--library','/unused','--hybrid-library','/unused',
            '--archive','/unused','--openssl','/unused','--output','/unused','--crypto-operation','dhparam']
        for operation in ['dhparam','curve','dtls-init']:
            with self.subTest(operation=operation):
                command[-1]=operation
                result=subprocess.run(command,text=True,capture_output=True)
                self.assertEqual(result.returncode,2)
                self.assertIn('--crypto-operation requires --crypto-allocation',result.stderr)


class TimedOutOutput(unittest.TestCase):
    def test_timeout_preserves_bytes_and_cannot_bless_a_pass(self):
        spec=importlib.util.spec_from_file_location('tls_differential',Path(__file__).with_name('differential.py'))
        differential=importlib.util.module_from_spec(spec);spec.loader.exec_module(differential)
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);library=root/'native.so';library.write_bytes(b'synthetic, never loaded')
            report=root/'report.json'
            argv=['differential','--source',str(root),'--reference',str(root),'--library',str(library),
                  '--hybrid-library',str(library),'--archive',str(root/'unused.a'),'--openssl',str(root),'--output',str(report),'--command-timeout','1']
            failure=subprocess.TimeoutExpired(['synthetic command'],1,output=b'partial original stdout\n',stderr=b'native stderr\n')
            with patch.object(sys,'argv',argv),patch.object(differential,'run_isolated',side_effect=failure):
                self.assertEqual(differential.main(),1)
            actual=json.loads(report.read_text());self.assertFalse(actual['passed'])
            self.assertEqual(actual['commands'][0]['returncode'],124)
            self.assertTrue(actual['commands'][0]['timed_out'])
            self.assertEqual(Path(actual['commands'][0]['log']).read_bytes(),b'partial original stdout\nnative stderr\n')


if __name__ == '__main__': unittest.main()
