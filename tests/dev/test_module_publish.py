"""A failed optional libtool copy cannot prevent publishing a completed library."""
import os
import json
import hashlib
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools.build_upstream import build_extension
from tools.build_hybrid import native_compilation, export_build
from tools.private_copy import copy_private


class ModulePublication(unittest.TestCase):
    def test_export_checks_artifacts_and_never_replaces_evidence(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'build/src'
            (source / 'modules').mkdir(parents=True)
            (source / 'modules/swoole.so').write_bytes(b'completed module')
            (source / 'Makefile').write_text('CC = cc\nCXXFLAGS = -O2\n')
            (source / 'config.h').write_text('#define CONFIGURED 1\n')
            (source.parent / 'build.log').write_text('cc --mode=compile -O2\n')
            manifest = source.parent / 'build-manifest.json'
            manifest.write_text(json.dumps({'build_mode': 'immutable', 'producer_source_ref': 'frozen-source', 'inputs': {},
                'extension_sha256': hashlib.sha256(b'completed module').hexdigest()}))
            destination = root / 'exports/first'
            result = export_build(manifest, source, destination, 'frozen-source')
            self.assertEqual(result['source_ref'], 'frozen-source')
            self.assertEqual(result['producer_source_ref'], 'frozen-source')
            self.assertEqual((destination / 'swoole.so').read_bytes(), b'completed module')
            with self.assertRaises(FileExistsError):
                export_build(manifest, source, destination, 'frozen-source')
            original_manifest = manifest.read_bytes()
            real_digest = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
            def replace_manifest(path):
                if Path(path).name == 'swoole.so':
                    manifest.write_bytes(original_manifest + b'\n')
                return real_digest(path)
            with patch('tools.build_hybrid.digest', side_effect=replace_manifest):
                with self.assertRaisesRegex(ValueError, 'differ from their manifest'):
                    export_build(manifest, source, root / 'exports/replaced-manifest', 'frozen-source')
            manifest.write_bytes(original_manifest)
            original_copy = copy_private
            def mutate_config(first, second):
                original_copy(first, second)
                if Path(first).name == 'config.h':
                    Path(first).write_text('concurrent source change')
            with patch('tools.build_hybrid.copy_private', side_effect=mutate_config):
                with self.assertRaisesRegex(ValueError, 'changed during export'):
                    export_build(manifest, source, root / 'exports/changed', 'frozen-source')
            self.assertFalse((root / 'exports/changed').exists())
            (source / 'modules/swoole.so').write_bytes(b'tampered module')
            with self.assertRaisesRegex(ValueError, 'differ from their manifest'):
                export_build(manifest, source, root / 'exports/tampered', 'frozen-source')

    def test_new_publication_refuses_unknown_or_different_producer_without_mutation(self):
        for origin in [None, 'previous-source']:
            with self.subTest(origin=origin), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                source = root / 'source'; source.mkdir()
                manifest = root / 'build-manifest.json'
                record = {'build_mode': 'immutable'}
                if origin is not None:
                    record['producer_source_ref'] = origin
                manifest.write_text(json.dumps(record))
                before = manifest.read_bytes()
                destination = root / 'not-created/export'
                with self.assertRaisesRegex(ValueError, 'unknown|cross-cohort'):
                    export_build(manifest, source, destination, 'current-source')
                self.assertEqual(manifest.read_bytes(), before)
                self.assertFalse(destination.parent.exists())
                self.assertEqual(list(source.iterdir()), [])

    def test_recipe_environment_does_not_replace_configured_compiler(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'config.h').write_text('#define CONFIGURED 1\n')
            (root / 'Makefile').write_text(
                'CC = ccache cc -std=gnu23\nCXXFLAGS = -g -O2\n'
                'test:\n\tCC="$(CC)" \\\n\tCXXFLAGS="$(CXXFLAGS)" php run-tests.php\n')
            log = root / 'build.log'
            log.write_text('')
            observed = native_compilation(root, log)['make_variables']
            self.assertEqual(observed['CC'], 'ccache cc -std=gnu23')
            self.assertEqual(observed['CXXFLAGS'], '-g -O2')

    def test_native_flags_are_bound_without_evaluating_make_expressions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            marker = root / 'must-not-exist'
            (root / 'config.h').write_text('#define CONFIGURED 1\n')
            makefile = root / 'Makefile'
            makefile.write_text('CXXFLAGS = -O2 $(shell touch ' + str(marker) + ')\n')
            log = root / 'build.log'
            log.write_text('/bin/sh libtool --mode=compile c++ -O2 -c caller.cc\n')
            first = native_compilation(root, log)
            self.assertFalse(marker.exists())
            self.assertEqual(first['compile_command_count'], 1)
            self.assertIn('$(shell', first['make_variables']['CXXFLAGS'])
            makefile.write_text('CXXFLAGS = -O0\n')
            changed = native_compilation(root, log)
            self.assertNotEqual(first['files_sha256']['Makefile'], changed['files_sha256']['Makefile'])
            log.write_text('/bin/sh libtool --mode=compile c++ -O0 -c caller.cc\n')
            self.assertNotEqual(first['compile_commands_sha256'], native_compilation(root, log)['compile_commands_sha256'])

    def test_complete_library_publishes_without_broken_install_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'ext-src').mkdir()
            (root / 'ext-src/php_swoole.lo').write_bytes(b'object')
            (root / 'Makefile').write_text(
                'all: modules/swoole.so\n'
                'swoole.la: ext-src/php_swoole.lo\n'
                '\tmkdir -p .libs\n'
                '\tprintf "linked\\000bytes" > .libs/swoole.so\n'
                '\ttouch swoole.la\n'
                'modules/swoole.so: swoole.la\n'
                '\t@echo "install copy deallocation failure" >&2; exit 1\n')
            broken = subprocess.run(['make', 'all'], cwd=root, capture_output=True)
            self.assertNotEqual(broken.returncode, 0)
            self.assertIn(b'install copy deallocation failure', broken.stderr)
            timings = {}
            with patch('tools.build_upstream.extension_configuration', return_value=([], os.environ.copy())), \
                 patch('tools.build_upstream.require_build_space'):
                build_extension(root, root / 'build.log', SimpleNamespace(jobs=1),
                                incremental=True, timings=timings)
            self.assertEqual((root / 'modules/swoole.so').read_bytes(), b'linked\0bytes')
            self.assertIn('publish_module', timings)

    def test_evicted_anchor_or_pic_object_rebuilds_before_linking(self):
        for descriptor_present in [False, True]:
            with self.subTest(descriptor_present=descriptor_present), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / 'ext-src').mkdir()
                if descriptor_present:
                    (root / 'ext-src/php_swoole.lo').write_text("pic_object='.libs/php_swoole.o'\nnon_pic_object='none'\n")
                (root / 'Makefile').write_text(
                    'swoole.la: ext-src/php_swoole.lo\n'
                    '\ttest -f ext-src/.libs/php_swoole.o\n'
                    '\tmkdir -p .libs; cp ext-src/.libs/php_swoole.o .libs/swoole.so\n'
                    '\ttouch swoole.la\n'
                    'ext-src/php_swoole.lo:\n'
                    '\tmkdir -p ext-src/.libs\n'
                    '\tprintf "rebuilt" > ext-src/.libs/php_swoole.o\n'
                    '\tprintf "descriptor" > ext-src/php_swoole.lo\n')
                with patch('tools.build_upstream.extension_configuration', return_value=([], os.environ.copy())), \
                     patch('tools.build_upstream.require_build_space'):
                    build_extension(root, root / 'build.log', SimpleNamespace(jobs=1), incremental=True)
                self.assertEqual((root / 'modules/swoole.so').read_bytes(), b'rebuilt')


if __name__ == '__main__':
    unittest.main()
