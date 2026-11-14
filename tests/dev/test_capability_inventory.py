"""Actual PHP tokenizer regression for source API scope, without executing stubs."""
import json
from pathlib import Path
import shutil
import subprocess
import unittest
from tools.capability_inventory import declarations, socket_availability
ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which('php'), 'PHP tokenizer required')
class Declarations(unittest.TestCase):
    def tokens(self, source):
        # Distribution builds ship tokenizer as a shared module, which -n does not load.
        loaded = 'tokenizer' in subprocess.check_output(['php', '-n', '-m'], text=True).split()
        result = subprocess.run(['php', '-n', *([] if loaded else ['-d', 'extension=tokenizer']),
                                 str(ROOT/'tools/php_api_inventory.php'), '--tokens'],
                                input=json.dumps({'fixture.stub.php': source}), capture_output=True, text=True, check=True)
        return json.loads(result.stdout)['fixture.stub.php']

    def test_namespace_and_class_scope_with_comment_string_and_reference_traps(self):
        source = '''<?php
namespace Outer {
 // class Fake { function missing() {} }
 interface Contract { public function &read(): string; }
 class Thing implements Contract {
   public function &read(): string { $s = "fake } class Ghost {"; return $s; }
   public function kind(): string { return Thing::class; }
 }
 function helper() { return Thing::class; }
}
namespace Second;
function next_one() {}
'''
        rows = declarations(self.tokens(source))
        self.assertEqual([(r['kind'], r['name']) for r in rows], [
            ('interface', 'Outer\\Contract'), ('method', 'Outer\\Contract::read'),
            ('class', 'Outer\\Thing'), ('method', 'Outer\\Thing::read'),
            ('method', 'Outer\\Thing::kind'), ('function', 'Outer\\helper'),
            ('function', 'Second\\next_one')])
        self.assertTrue(all(row['line'] > 0 for row in rows))

    def test_constant_visibility_inheritance_finality_and_types_without_value_evaluation(self):
        script = r'''$inventory = $argv[1]; $argv[1] = '--tokens'; ob_start(); require $inventory; ob_end_clean();
class ParentFixture { final public const SEALED = 7; protected const HIDDEN = 8; }
class ChildFixture extends ParentFixture { private const PRIVATE_VALUE = 9; }
echo json_encode(swr_inventory_class_constants(new ReflectionClass(ChildFixture::class)), JSON_THROW_ON_ERROR);
'''
        result = subprocess.run(['php', '-n', '-r', script, '--', str(ROOT/'tools/php_api_inventory.php')],
                                input='{}', capture_output=True, text=True, check=True)
        rows = json.loads(result.stdout)
        self.assertEqual(rows['SEALED'], dict(declared_by='ParentFixture', visibility='public', final=True,
                                             type=rows['SEALED']['type']))
        self.assertIn(rows['SEALED']['type'], [None, ''])
        self.assertEqual(rows['HIDDEN']['visibility'], 'protected')
        self.assertEqual(rows['PRIVATE_VALUE']['visibility'], 'private')
        self.assertEqual(rows['PRIVATE_VALUE']['declared_by'], 'ChildFixture')

    def test_stub_constants_keep_namespace_and_class_owners_without_evaluating_values(self):
        source = r"""<?php namespace Constants {
use const Outside\IMPORTED;
use Outside\{const GROUPED};
class Item {
    public const string LABEL = "comma, equals= braces{}", SECOND = [1, [2, 3]];
}
const FIRST = UNKNOWN, SECOND = ["x" => [1, 2]];
}"""
        rows = declarations(self.tokens(source))
        self.assertEqual([(row['kind'], row['name']) for row in rows], [
            ('class', 'Constants\\Item'),
            ('class_constant', 'Constants\\Item::LABEL'),
            ('class_constant', 'Constants\\Item::SECOND'),
            ('constant', 'Constants\\FIRST'), ('constant', 'Constants\\SECOND')])
        self.assertTrue(all(row['line'] > 0 for row in rows))

    def test_unclosed_scope_and_anonymous_class_are_explicit_errors(self):
        for text in ['<?php namespace Broken { class Bad {', '<?php $x = new class {};']:
            with self.subTest(text=text), self.assertRaises(ValueError):
                declarations(self.tokens(text))


class SocketAvailability(unittest.TestCase):
    def test_php_extension_does_not_imply_swoole_adapter(self):
        self.assertEqual(socket_availability(dict(classes={}, loaded_extensions=['sockets'])), {
            'php_sockets_extension_loaded': True, 'swoole_client_get_socket_method': False})

    def test_public_adapter_uses_canonical_class_and_case_insensitive_method(self):
        api = dict(classes={'swoole_client': dict(canonical_name='Swoole\\Client',
                   methods={'GETSOCKET': dict(visibility='public')})}, loaded_extensions=[])
        self.assertEqual(socket_availability(api), {
            'php_sockets_extension_loaded': False, 'swoole_client_get_socket_method': True})
        api['classes']['swoole_client']['methods']['GETSOCKET']['visibility'] = 'protected'
        self.assertFalse(socket_availability(api)['swoole_client_get_socket_method'])


class SurfaceComparison(unittest.TestCase):
    def pair(self):
        import copy
        api = dict(php='8.2.0', zts=False, os='Linux', integer_bytes=8,
                   classes={'Swoole\\Fixture': {'properties': {'value': {'type': 'int'}}, 'methods': {}}},
                   functions={}, constants={'ORIGINAL': 7}, ini={}, loaded_extensions=['swoole'])
        native = dict(engine=dict(rust_bridge_abi=None, rust_feature_bits=0), api=api)
        selected = dict(engine=dict(rust_bridge_abi=1, rust_feature_bits=2), api=copy.deepcopy(api))
        selected['api']['constants'].update(SWOOLE_RS_ABI_VERSION=1, SWOOLE_RS_FEATURE_BITS=2)
        return [native, selected]

    def test_exact_markers_are_separate_and_input_evidence_is_unchanged(self):
        import copy
        from tools.capability_inventory import compare_api_surfaces
        pair = self.pair()
        before = copy.deepcopy(pair)
        result = compare_api_surfaces(pair)
        self.assertTrue(result['passed'])
        self.assertEqual(result['diagnostic_constants'], {'SWOOLE_RS_ABI_VERSION': 1, 'SWOOLE_RS_FEATURE_BITS': 2})
        self.assertEqual(pair, before)

    def test_property_constant_and_missing_api_changes_are_reported(self):
        from tools.capability_inventory import compare_api_surfaces
        pair = self.pair()
        pair[1]['api']['classes']['Swoole\\Fixture']['properties']['value']['type'] = 'string'
        del pair[1]['api']['constants']['ORIGINAL']
        pair[0]['api']['classes']['Swoole\\Fixture']['constant_metadata'] = {'VALUE': {'visibility': 'public'}}
        pair[1]['api']['classes']['Swoole\\Fixture']['constant_metadata'] = {'VALUE': {'visibility': 'protected'}}
        pair[1]['api']['functions']['unexpected'] = {'parameters': []}
        result = compare_api_surfaces(pair)
        self.assertFalse(result['passed'])
        self.assertEqual({tuple(row['path']) for row in result['differences']}, {
            ('classes', 'Swoole\\Fixture', 'properties', 'value', 'type'),
            ('constants', 'ORIGINAL'), ('functions', 'unexpected'),
            ('classes', 'Swoole\\Fixture', 'constant_metadata', 'VALUE', 'visibility')})

    def test_mismatched_profiles_and_wrong_bridge_markers_cannot_compare_green(self):
        from tools.capability_inventory import compare_api_surfaces
        for field, value in [('php', '8.5.0'), ('zts', True), ('os', 'Darwin'), ('integer_bytes', 4)]:
            pair = self.pair()
            pair[1]['api'][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'same PHP'):
                compare_api_surfaces(pair)
        pair = self.pair()
        pair[1]['api']['constants']['SWOOLE_RS_FEATURE_BITS'] = 0
        with self.assertRaisesRegex(ValueError, 'marker constants'):
            compare_api_surfaces(pair)

    def test_main_retains_first_error_warning_or_non_json_without_comparing(self):
        import tempfile
        from unittest.mock import Mock, patch
        from tools import capability_inventory as tool
        for code, stdout, stderr in [(255, 'first fatal output', ''), (0, '{}', 'first warning'), (0, 'not JSON', '')]:
            with self.subTest(code=code, stdout=stdout), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / 'report.json'
                engine = Mock()
                engine.command.return_value = ['fixture-reflection']
                engine.metadata.return_value = {'name': 'first'}
                with patch.object(tool, 'census', return_value={}), patch.object(tool, 'parse_engines', return_value=[engine, engine]), \
                     patch.object(tool.subprocess, 'run', return_value=subprocess.CompletedProcess(['fixture-reflection'], code, stdout, stderr)) as run, \
                     patch('sys.argv', ['capability_inventory', '--source', directory, '--engine', 'first=fixture', '--require-same-api', '--output', str(output)]):
                    self.assertEqual(tool.main(), 1)
                report = json.loads(output.read_text())
                self.assertNotIn('api_comparison', report)
                self.assertEqual(len(report['configured_reflection']), 1)
                self.assertEqual(report['configured_reflection'][0]['stdout'], stdout)
                self.assertEqual(report['configured_reflection'][0]['stderr'], stderr)
                self.assertEqual(run.call_count, 1)
