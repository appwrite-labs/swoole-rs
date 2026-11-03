#!/usr/bin/env python3
"""Validate frozen benchmark schema compatibility without executing timings."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('memory_benchmark', Path(__file__).with_name('benchmark.py'))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)
OLD = ['fixed_ns', 'heap_ns', 'channel_ns', 'ring_ns', 'global_ns', 'table_ns']
NEW = OLD + ['table_clear8_ns', 'table_clear33_ns']
CONSTRUCTION = NEW + ['fixed_construct_owned_ns', 'fixed_construct_borrowed_ns']
COLUMNS = CONSTRUCTION + ['table_schema17_ns', 'table_columns65_ns']
NAMES = COLUMNS + ['table_names65_ns', 'table_long_names65_ns']


class FrozenCases(unittest.TestCase):
    def run_recipe(self, declared, observed):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            marker = root / 'marker'
            marker.write_bytes(b'frozen input')
            digest = hashlib.sha256(marker.read_bytes()).hexdigest()
            manifest = {'source_snapshot': str(root), 'frozen_sha256': {'marker': digest},
                        'variants': {name: {'binary': str(marker), 'sha256': digest, 'extension': str(marker), 'extension_sha256': digest,
                                           'command': ['mock-only', name]} for name in ['upstream', 'rust']}}
            if declared is not None:
                manifest['timing_cases'] = declared
            (root / 'manifest.json').write_text(json.dumps(manifest))
            output = root / 'result.json'
            commands = []

            def observation(command, **kwargs):
                # A real subprocess here would violate this test's contract.
                self.assertEqual(command[0], 'mock-only')
                commands.append(command)
                value = 10 if command[1] == 'upstream' else 20
                body = dict(loops=10, checksum=7, **{case: value for case in observed})
                return SimpleNamespace(returncode=0, stdout=json.dumps(body) + '\n', stderr='')

            arguments = ['benchmark.py', '--prepared', str(root), '--trials', '3',
                         '--loops', '10', '--output', str(output)]
            with patch.object(benchmark.sys, 'argv', arguments), \
                    patch.object(benchmark, 'run_isolated', side_effect=observation), \
                    contextlib.redirect_stdout(io.StringIO()):
                code = benchmark.main()
            return code, json.loads(output.read_text()), commands

    def test_legacy_six_case_frozen_recipe_remains_compatible(self):
        code, report, commands = self.run_recipe(None, OLD)
        self.assertEqual(code, 0)
        self.assertEqual(set(report['rust_over_upstream']), set(OLD))
        self.assertTrue(all(value == 2 for value in report['rust_over_upstream'].values()))
        self.assertEqual([command[1] for command in commands],
                         ['upstream', 'rust', 'rust', 'upstream', 'upstream', 'rust'])

    def test_explicit_eight_case_recipe_keeps_both_clear_workloads(self):
        code, report, _ = self.run_recipe(NEW, NEW)
        self.assertEqual(code, 0)
        self.assertEqual(set(report['rust_over_upstream']), set(NEW))

    def test_constructor_recipe_preserves_explicit_setup_costs(self):
        code, report, _ = self.run_recipe(CONSTRUCTION, CONSTRUCTION)
        self.assertEqual(code, 0)
        self.assertEqual(set(report['rust_over_upstream']), set(CONSTRUCTION))

    def test_column_lifecycle_and_cursor_recipe_remains_explicit(self):
        code, report, _ = self.run_recipe(COLUMNS, COLUMNS)
        self.assertEqual(code, 0)
        self.assertEqual(set(report['rust_over_upstream']), set(COLUMNS))

    def test_name_copy_consumer_recipes_are_explicit(self):
        code, report, _ = self.run_recipe(NAMES, NAMES)
        self.assertEqual(code, 0)
        self.assertEqual(set(report['rust_over_upstream']), set(NAMES))

    def test_missing_or_unadvertised_cases_fail_closed(self):
        for declared, observed in [(NEW, OLD), (None, NEW), (CONSTRUCTION, NEW), (NEW, CONSTRUCTION), (COLUMNS, CONSTRUCTION), (CONSTRUCTION, COLUMNS), (NAMES, COLUMNS), (COLUMNS, NAMES)]:
            with self.subTest(declared=declared):
                code, report, _ = self.run_recipe(declared, observed)
                self.assertEqual(code, 1)
                self.assertFalse(report['completed'])
                self.assertTrue(report['errors'])


if __name__ == '__main__':
    unittest.main()
