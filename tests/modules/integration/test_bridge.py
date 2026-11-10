#!/usr/bin/env python3
"""Exercise the real C ABI and PHP module startup.

Build the reference and the Cargo extension (tools/build_ext.py) first, then run: python3 tests/modules/integration/test_bridge.py
All compilations write only to a temporary directory.
"""
import argparse
import os
import re
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.build_ext import headers_for  # noqa: E402
from tools.check_migration import MODULES  # noqa: E402

OWNED = ROOT / 'native/swoole-src'


def run(command, *, cwd=ROOT, check=True):
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True, timeout=120)
    if check and result.returncode:
        raise AssertionError(f'{shlex.join(map(str, command))}\n{result.stdout}{result.stderr}')
    return result


class BridgeTests(unittest.TestCase):
    def php(self, extension, *arguments):
        dependencies = [argument for name in ARGS.php_extension for argument in ['-d', f'extension={name}']]
        return run([ARGS.php, '-n', *dependencies, '-d', f'extension={extension}', *arguments], check=False)

    def test_c_and_cpp_consumers(self):
        source = '''#include "swoole_rs.h"
int main(void) {
    return SWR_ABI_VERSION != 1 || swr_v1_abi_version() != 1 || swr_v1_feature_bits() != EXPECTED_FEATURE_BITS;
}
'''.replace('EXPECTED_FEATURE_BITS', str(ARGS.feature_bits))
        native = ['-lm']
        if sys.platform.startswith('linux'):
            native += ['-ldl', '-lpthread', '-lrt', '-lutil']
        with tempfile.TemporaryDirectory(prefix='swoole-rs-abi-') as directory:
            temporary = Path(directory)
            for language, variable, default in [('c', 'CC', 'cc'), ('cc', 'CXX', 'c++')]:
                with self.subTest(language=language):
                    path = temporary / f'consumer.{language}'
                    path.write_text(source)
                    executable = temporary / f'consumer-{language}'
                    run(shlex.split(os.environ.get(variable, default)) + [str(path), '-I', str(ROOT / 'native/include'),
                        str(ARGS.archive), *native, '-o', str(executable)])
                    run([str(executable)])

    def test_selected_abi_exports(self):
        # The Cargo build has every module, so it declares every ABI header the hybrid would.
        headers = [header for header in headers_for(MODULES)
                   # The hook headers rewrite libc calls with function-like macros for the
                   # sources that include them; they declare no ABI and break later headers.
                   if not header.name.endswith('_hook.h')]
        # Inventory the declarations selected by the actual target/compiler and
        # configured SDK. Raw text includes unavailable OS backends as well.
        source = ARGS.source
        includes = shlex.split(run([ARGS.php_config, '--includes']).stdout)
        directories = sorted({str(header.parent) for header in headers})
        # Configure-gated declarations (SW_STDEXT, SW_THREAD, SW_USE_CURL, ...) follow the
        # config.h the build used, which build_ext.py writes beside swoole.so.
        with tempfile.TemporaryDirectory(prefix='swoole-rs-export-inventory-') as directory:
            inventory = Path(directory) / 'headers.cc'
            inventory.write_text(''.join(f'#include "{header}"\n' for header in headers))
            text = run(shlex.split(os.environ.get('CXX', 'c++')) + [
                '-E', '-P', '-x', 'c++', '-include', str(ARGS.extension.parent / 'config.h'), *[f'-I{path}' for path in directories],
                # crates/ext's order: native/swoole-src's copies ahead of the pin's files.
                '-I', str(OWNED), '-I', str(source), '-I', str(source / 'ext-src'),
                '-I', str(OWNED / 'thirdparty'), '-I', str(source / 'thirdparty'), *includes, str(inventory)]).stdout
        text = re.sub(r'/\*.*?\*/|//[^\n]*', '', text, flags=re.S)
        expected = set(re.findall(r'\b(swr_v[0-9]+_\w+)\s*\(', text))
        self.assertTrue(expected, 'No selected ABI declarations were checked')
        options = ['-gU'] if sys.platform == 'darwin' else ['-D', '--defined-only']
        symbols = run(['nm', *options, str(ARGS.extension)]).stdout
        actual = set(re.findall(r'\b_?(swr_v[0-9]+_\w+)\b', symbols))
        self.assertFalse(expected - actual, 'Selected ABI exports missing: ' + ', '.join(sorted(expected - actual)))

    def test_public_http_status_array_export(self):
        if not ARGS.feature_bits & 256:
            self.skipTest('HTTP module is not selected')
        # The existing public C++ data symbol must survive static archive
        # extraction and production DSO export filtering, not just a mini DSO.
        symbol = '_ZN6swoole11http_server19list_of_status_codeE'
        if sys.platform == 'darwin':
            symbol = '_' + symbol
        options = ['-gU'] if sys.platform == 'darwin' else ['-D', '--defined-only']
        for label, extension in [('native', ARGS.reference), ('selected', ARGS.extension)]:
            with self.subTest(engine=label):
                symbols = run(['nm', *options, str(extension)]).stdout
                definitions = [line.split() for line in symbols.splitlines()
                               if line.split() and line.split()[-1] == symbol]
                self.assertEqual(len(definitions), 1,
                                 f'{label} production extension lacks the unique public status array: {symbol}')
                self.assertIn(definitions[0][-2], {'B', 'D', 'G', 'S'},
                              f'{label} public status array is not mutable exported data')

    def test_module_information_exports_follow_profile(self):
        options = ['-gU'] if sys.platform == 'darwin' else ['-D', '--defined-only']
        defined = run(['nm', *options, str(ARGS.extension)]).stdout
        symbols = set(re.findall(r'\b_?(swr_v[0-9]+_\w+)\b', defined))
        self.assertEqual('swr_v1_phpswoole_minfo' in symbols, bool(ARGS.feature_bits & 16777216))

    def test_rust_internal_symbols_are_hidden(self):
        # config.m4's version script keeps Rust's v0-mangled symbols local, so
        # --gc-sections can drop the code nothing reaches (#2242).
        if not sys.platform.startswith('linux'):
            self.skipTest('the version script is Linux-only')
        exported = [line.split()[-1] for line in run(['nm', '-D', '--defined-only', str(ARGS.extension)]).stdout.splitlines()
                    if line.split() and line.split()[-1].startswith('_R')]
        self.assertFalse(exported, 'Rust internal symbols exported: ' + ', '.join(exported[:10]))

    def test_reference_has_no_bridge_marker(self):
        result = self.php(ARGS.reference, '-r', 'echo extension_loaded("swoole") ? "loaded\\n" : "missing\\n"; '
                          'echo defined("SWOOLE_RS_ABI_VERSION") || defined("SWOOLE_RS_FEATURE_BITS") ? "bridge\\n" : "upstream\\n";')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, 'loaded\nupstream\n')
        self.assertNotIn('Rust migration bridge', self.php(ARGS.reference, '--ri', 'swoole').stdout)

    def test_extension_markers_and_phpinfo(self):
        result = self.php(ARGS.extension, '-r', 'echo SWOOLE_RS_ABI_VERSION, ":", SWOOLE_RS_FEATURE_BITS, "\\n";')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, f'1:{ARGS.feature_bits}\n')
        info = self.php(ARGS.extension, '--ri', 'swoole')
        self.assertEqual(info.returncode, 0, info.stdout + info.stderr)
        self.assertIn(f'Rust migration bridge => ABI 1, features {ARGS.feature_bits}', info.stdout)



if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='Load before swoole, e.g. sockets')
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--reference', type=Path, default=ROOT / 'target/upstream/src/modules/swoole.so')
    parser.add_argument('--extension', type=Path, default=ROOT / 'target/ext/swoole.so', help='The Cargo-built swoole.so')
    parser.add_argument('--source', type=Path, default=ROOT / 'target/ext/src', help='The pin it was built from')
    parser.add_argument('--archive', type=Path, help='A Zend-free libswoole_rs_ffi.a for the C and C++ consumers')
    parser.add_argument('--startup-only', action='store_true', help='Positive startup/flag gates; the C and C++ consumers remain integration-only')
    ARGS = parser.parse_args()
    # Every module: crates/error's swr_v1_feature_bits.
    ARGS.feature_bits = sum(bit for _, bit in MODULES.values())
    if not ARGS.startup_only and ARGS.archive is None:
        parser.error('--archive is required unless --startup-only')
    for path in [ARGS.reference, ARGS.extension, *([ARGS.archive] if ARGS.archive else [])]:
        if not path.is_file():
            parser.error(f'Missing built artifact: {path}; build reference and extension first')
    tests = ['BridgeTests.test_reference_has_no_bridge_marker', 'BridgeTests.test_extension_markers_and_phpinfo',
             'BridgeTests.test_selected_abi_exports', 'BridgeTests.test_public_http_status_array_export',
             'BridgeTests.test_module_information_exports_follow_profile',
             'BridgeTests.test_rust_internal_symbols_are_hidden'] if ARGS.startup_only else []
    unittest.main(argv=[sys.argv[0], *tests], verbosity=2)
