"""Bounded source/fixture checks; clocks are poisoned during Python preparation."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(Path(__file__).parent))
spec=importlib.util.spec_from_file_location('prepare_utilities',Path(__file__).with_name('prepare_benchmark.py'))
prepare=importlib.util.module_from_spec(spec);spec.loader.exec_module(prepare)


class PreparationGuards(unittest.TestCase):
    def test_manifest_source_mismatch_refuses_before_compilation_or_elapsed_clock(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);archive=root/'selected.a';archive.write_bytes(b'fixture archive')
            manifest=root/'manifest.json'
            selected={'build_mode':'immutable','rust_compilation':{'command':['cargo','build','--release'],'artifact':{'profile':{'opt_level':'3','debug_assertions':False,'test':False}}},'inputs':{'backend':'rust','modules':['utilities'],'upstream_commit':prepare.PIN,
                      'rust_archive_sha256':prepare.sha(archive),
                      'headers_sha256':{'native/utilities/swoole_rs_utilities.h':prepare.sha(ROOT/'native/utilities/swoole_rs_utilities.h')},
                      'rust_sources_sha256':{'crates/utilities/src/lib.rs':'0'*64}}}
            manifest.write_text(json.dumps(selected));output=root/'report.json'
            args=['prepare.py','--source',str(root),'--archive',str(archive),'--build-manifest',str(manifest),'--output',str(output)]
            with patch.object(sys,'argv',args),patch.object(prepare,'require_build_space'), \
                 patch.object(prepare,'run_isolated') as execute, \
                 patch.object(time,'monotonic',side_effect=AssertionError('elapsed clock read')), \
                 patch.object(time,'perf_counter',side_effect=AssertionError('elapsed clock read')),contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(prepare.main(),1)
            report=json.loads(output.read_text())
            self.assertFalse(report['passed']);self.assertIn('Selected utility source mismatch',report['error'])
            self.assertEqual(report['checks'],[]);execute.assert_not_called()

    def test_partial_maps_refuse_before_source_preparation_and_full_map_reaches_control(self):
        sources={str(p.relative_to(ROOT)):prepare.sha(p) for p in
                 [ROOT/'crates/utilities/Cargo.toml', *(ROOT/'crates/utilities/src').rglob('*.rs')]}
        patches=[dict(path='migration/patches/utilities/'+p['path'],sha256=p['sha256']) for p in prepare.module_patches(ROOT/'migration/patches/utilities')]
        for case in ['missing-source','missing-patch','duplicate-patch','complete']:
            with self.subTest(case=case),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);archive=root/'selected.a';archive.write_bytes(b'fixture archive')
                (root/'config.h').write_text('/* synthetic metadata only */\n')
                selected={'build_mode':'immutable','rust_compilation':{'command':['cargo','build','--release'],'artifact':{'profile':{'opt_level':'3','debug_assertions':False,'test':False}}},'inputs':{'backend':'rust','modules':['utilities'],'upstream_commit':prepare.PIN,
                          'rust_archive_sha256':prepare.sha(archive),
                          'headers_sha256':{'native/utilities/swoole_rs_utilities.h':prepare.sha(ROOT/'native/utilities/swoole_rs_utilities.h')},
                          'rust_sources_sha256':dict(sources),'patches':[dict(p) for p in patches]}}
                if case=='missing-source':del selected['inputs']['rust_sources_sha256']['crates/utilities/src/lib.rs']
                if case=='missing-patch':selected['inputs']['patches'].pop()
                if case=='duplicate-patch':selected['inputs']['patches'].append(dict(patches[0]))
                manifest=root/'manifest.json';manifest.write_text(json.dumps(selected));output=root/'report.json'
                args=['prepare.py','--source',str(root),'--archive',str(archive),'--build-manifest',str(manifest),'--output',str(output)]
                with patch.object(sys,'argv',args),patch.object(prepare,'require_build_space'), \
                     patch.object(prepare,'run_isolated') as execute, \
                     patch.object(prepare,'prepare_oracles',side_effect=RuntimeError('Controlled preflight stop')) as oracle, \
                     patch.object(time,'monotonic',side_effect=AssertionError('elapsed clock read')), \
                     patch.object(time,'perf_counter',side_effect=AssertionError('elapsed clock read')),contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(prepare.main(),1)
                report=json.loads(output.read_text());self.assertFalse(report['passed']);self.assertEqual(report['checks'],[]);execute.assert_not_called()
                if case=='complete':
                    self.assertIn('Controlled preflight stop',report['error']);oracle.assert_called_once()
                else:
                    self.assertIn('Require complete current utility',report['error']);oracle.assert_not_called()

    def test_workloads_remain_distinct_and_clock_calls_stay_explicit(self):
        workloads=prepare.WORKLOADS+prepare.LOGGER_WORKLOADS
        self.assertEqual(len(workloads),40);self.assertEqual(len(workloads),len(set(workloads)))
        for name in ['benchmark.cc','benchmark_logger.cc']:
            text=Path(__file__).with_name(name).read_text()
            self.assertNotIn('system_clock::now()',text)
            self.assertIn('"--measure"',text)
            self.assertEqual(text.count('steady_clock::now()'),2)
        self.assertIn('std::chrono::',Path(__file__).with_name('prepare_benchmark.py').read_text())


if __name__=='__main__':unittest.main()
