"""Legacy same-cohort exports stay readable; reuse must not mix cohorts or aliases."""
import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from tools import dev


class ArtifactReuse(unittest.TestCase):
    def fixture(self, root):
        args = SimpleNamespace(artifacts_from=root/'reports', lane='integrate', module=['memory'],
                               all_modules=False, stdext=False, fix_halfclose=False)
        for label, variant in [('selected', dev.build_variant(args)), ('native', 'upstream')]:
            folder = args.artifacts_from/(label+'-export')
            alias = root/'target/hybrid'/variant
            folder.mkdir(parents=True)
            (alias/'modules').mkdir(parents=True)
            manifest = dict(build_mode='immutable', configure=[],
                            inputs=dict(modules=sorted(dev.MODULES) if label=='selected' else [], fix_halfclose=False))
            values = {'swoole.so':label.encode(), 'build-manifest.json':json.dumps(manifest).encode()}
            for name, value in values.items():
                (folder/name).write_bytes(value)
                (alias/('modules/swoole.so' if name=='swoole.so' else name)).write_bytes(value)
            receipt = dict(source_ref='frozen', files={n:dict(sha256=hashlib.sha256(v).hexdigest()) for n,v in values.items()})
            (folder/'export.json').write_text(json.dumps(receipt))
        return args

    def test_same_cohort_passes_and_stale_or_changed_inputs_refuse(self):
        for mutation in ['none', 'source', 'export', 'alias', 'configuration']:
            with self.subTest(mutation=mutation), TemporaryDirectory() as temporary:
                root=Path(temporary);args=self.fixture(root)
                if mutation=='export': (args.artifacts_from/'selected-export/swoole.so').write_bytes(b'changed')
                if mutation=='alias': (root/'target/hybrid'/dev.build_variant(args)/'modules/swoole.so').write_bytes(b'changed')
                if mutation=='configuration': args.fix_halfclose=True
                with patch.object(dev,'ROOT',root), patch.object(dev.subprocess,'check_output',return_value='different' if mutation=='source' else 'frozen'):
                    if mutation=='none': self.assertEqual(set(dev.verify_artifact_report(args)),{'native','selected'})
                    else:
                        with self.assertRaises(ValueError): dev.verify_artifact_report(args)

    def test_known_producer_must_match_receipt_manifest_and_current_cohort(self):
        for label in ['native', 'selected']:
            for receipt_origin, manifest_origin in [('frozen', 'frozen'), ('older', 'frozen'),
                                                      ('frozen', 'older'), (None, 'frozen'), ('frozen', None)]:
                with self.subTest(label=label, receipt=receipt_origin, manifest=manifest_origin), TemporaryDirectory() as temporary:
                    root = Path(temporary); args = self.fixture(root)
                    folder = args.artifacts_from / (label + '-export')
                    manifest_path = folder / 'build-manifest.json'
                    manifest = json.loads(manifest_path.read_text())
                    if manifest_origin is not None:
                        manifest['producer_source_ref'] = manifest_origin
                    manifest_path.write_text(json.dumps(manifest))
                    receipt_path = folder / 'export.json'
                    receipt = json.loads(receipt_path.read_text())
                    if receipt_origin is not None:
                        receipt['producer_source_ref'] = receipt_origin
                    receipt['files']['build-manifest.json']['sha256'] = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
                    receipt_path.write_text(json.dumps(receipt))
                    variant = 'upstream' if label == 'native' else dev.build_variant(args)
                    (root / 'target/hybrid' / variant / 'build-manifest.json').write_bytes(manifest_path.read_bytes())
                    with patch.object(dev, 'ROOT', root), patch.object(dev.subprocess, 'check_output', return_value='frozen'):
                        if receipt_origin == manifest_origin == 'frozen':
                            self.assertEqual(set(dev.verify_artifact_report(args)), {'native', 'selected'})
                        else:
                            with self.assertRaisesRegex(ValueError, 'producer source differs'):
                                dev.verify_artifact_report(args)

    def test_loaded_minimal_profiles_capture_both_producers(self):
        for modules, phase in [(['phpcoroutines'], 'phpcoroutines-tracer'),
                               (['http'], 'http-rewrites')]:
            with self.subTest(modules=modules):
                args=SimpleNamespace(lane='integrate', module=modules, all_modules=False,
                    stdext=False, fix_halfclose=False, php='php', php_extension=[], jobs=1,
                    test=None, case=None, benchmark=False, compare=False)
                reports=Path('/private/immutable-run')
                with patch.object(dev.subprocess,'check_output',return_value='-I/fixture'):
                    commands=dict(dev.plan(args,reports))
                self.assertIn(phase,commands)
                for name,label in [('reference-build','native'),('build','selected')]:
                    command=commands[name]
                    self.assertEqual(command[command.index('--export')+1],str(reports/(label+'-export')))
