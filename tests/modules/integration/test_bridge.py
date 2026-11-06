#!/usr/bin/env python3
"""Exercise the real C ABI and PHP module startup (including an incompatible ABI).

Build reference/hybrid first, then run: python3 tests/modules/integration/test_bridge.py
All compilations and the incompatible relink write only to a temporary directory.
"""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.private_copy import copy_private


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
        build = ARGS.hybrid.resolve().parents[2]
        manifest = json.loads((build / 'build-manifest.json').read_text())
        modules = set(manifest['inputs']['modules'])
        headers = []
        for relative, digest in manifest['inputs']['headers_sha256'].items():
            path = Path(relative)
            if path.parent == Path('native/include') and path.stem != 'swoole_rs':
                if path.stem.removeprefix('swoole_rs_') not in modules:
                    continue
            header = build / 'rust-prefix/include' / path.name
            content = header.read_bytes()
            self.assertEqual(hashlib.sha256(content).hexdigest(), digest, str(header))
            headers.append(header)
        # Inventory the declarations selected by the actual target/compiler and
        # configured SDK. Raw text includes unavailable OS backends as well.
        source = build / 'src'
        includes = shlex.split(run([ARGS.php_config, '--includes']).stdout)
        flags = shlex.split(manifest['inputs'].get('effective_cppflags', ''))
        for option in manifest.get('configure', []):
            if option.startswith('--with-openssl-dir='):
                flags += ['-I', str(Path(option.split('=', 1)[1]) / 'include')]
        with tempfile.TemporaryDirectory(prefix='swoole-rs-export-inventory-') as directory:
            inventory = Path(directory) / 'headers.cc'
            inventory.write_text(''.join(f'#include "{header}"\n' for header in headers))
            text = run(shlex.split(os.environ.get('CXX', 'c++')) + [
                '-E', '-P', '-x', 'c++', '-include', str(source / 'config.h'),
                '-I', str(build / 'rust-prefix/include'), '-I', str(source),
                '-I', str(source / 'include'), '-I', str(source / 'ext-src'),
                '-I', str(source / 'thirdparty'), *includes, *flags, str(inventory)]).stdout
        text = re.sub(r'/\*.*?\*/|//[^\n]*', '', text, flags=re.S)
        expected = set(re.findall(r'\b(swr_v[0-9]+_\w+)\s*\(', text))
        self.assertTrue(expected, 'No selected ABI declarations were checked')
        options = ['-gU'] if sys.platform == 'darwin' else ['-D', '--defined-only']
        symbols = run(['nm', *options, str(ARGS.hybrid)]).stdout
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
        for label, extension in [('native', ARGS.reference), ('selected', ARGS.hybrid)]:
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
        defined = run(['nm', *options, str(ARGS.hybrid)]).stdout
        symbols = set(re.findall(r'\b_?(swr_v[0-9]+_\w+)\b', defined))
        self.assertEqual('swr_v1_phpswoole_minfo' in symbols, bool(ARGS.feature_bits & 16777216))
        options = ['-u'] if sys.platform == 'darwin' else ['-D', '--undefined-only']
        undefined = run(['nm', *options, str(ARGS.hybrid)]).stdout
        for effect in ['swr_cpp_phpswoole_info_ftp', 'swr_cpp_phpswoole_info_ssh2']:
            self.assertNotIn(effect, undefined, 'Module information imports an unavailable native SDK effect')

    def test_reference_has_no_bridge_marker(self):
        result = self.php(ARGS.reference, '-r', 'echo extension_loaded("swoole") ? "loaded\\n" : "missing\\n"; '
                          'echo defined("SWOOLE_RS_ABI_VERSION") || defined("SWOOLE_RS_FEATURE_BITS") ? "bridge\\n" : "upstream\\n";')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, 'loaded\nupstream\n')
        self.assertNotIn('Rust migration bridge', self.php(ARGS.reference, '--ri', 'swoole').stdout)

    def test_hybrid_markers_and_phpinfo(self):
        result = self.php(ARGS.hybrid, '-r', 'echo SWOOLE_RS_ABI_VERSION, ":", SWOOLE_RS_FEATURE_BITS, "\\n";')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(result.stdout, f'1:{ARGS.feature_bits}\n')
        info = self.php(ARGS.hybrid, '--ri', 'swoole')
        self.assertEqual(info.returncode, 0, info.stdout + info.stderr)
        self.assertIn(f'Rust migration bridge => ABI 1, features {ARGS.feature_bits}', info.stdout)

    def require_space(self, temporary):
        for path in [ROOT, ARGS.archive.parent, temporary]:
            free = shutil.disk_usage(path).free
            self.assertGreaterEqual(free, 4294967296, f'Unchanged 4GiB guard: {path}, free={free}')

    def test_php_rejects_incompatible_archive(self):
        # Ask make for its expanded link recipe without executing any build step.
        # Existing .lo inputs remain read-only; libtool runs inside our scratch tree.
        source = ARGS.hybrid.resolve().parents[1]
        recipe = run(['make', '-n', '-W', 'ext-src/php_swoole_cxx.lo', 'swoole.la'], cwd=source).stdout
        lines = [line for line in recipe.splitlines() if '--mode=link' in line and '-o swoole.la' in line]
        self.assertEqual(len(lines), 1, 'Expected one expanded libtool link recipe: ' + recipe)
        command = shlex.split(lines[0])
        self.assertEqual(command.count('-lswoole_rs_php'), 1, 'Rust archive missing from link recipe')
        cases = [(999, 0, 'Rust bridge ABI mismatch: expected 1, got 999')]
        # Hide the real queries once (LTO merges them into objects with other code, so
        # they cannot be removed as members). Each case gets an independent copy of that real archive.
        with tempfile.TemporaryDirectory(prefix='swoole-rs-abi-preparation-') as directory:
            preparation = Path(directory)
            self.require_space(preparation)
            php_archive = ARGS.archive.with_name('libswoole_rs_php.a')
            prepared = preparation / php_archive.name
            copy_private(php_archive, prepared)
            run(['objcopy', '--localize-symbol=swr_v1_abi_version', '--localize-symbol=swr_v1_feature_bits',
                 str(prepared)])
            command = [str(source / argument) if argument.endswith('.lo') and not Path(argument).is_absolute()
                       else argument for argument in command]
            archive_index = command.index('-lswoole_rs_php')
            output_index, rpath_index = command.index('-o') + 1, command.index('-rpath') + 1
            stats_indices = [index for index, argument in enumerate(command) if argument.startswith('CCACHE_STATSLOG=')]
            for abi, bits, message in cases:
                with self.subTest(abi=abi, feature_bits=bits):
                    with tempfile.TemporaryDirectory(prefix='swoole-rs-bad-abi-') as directory:
                        temporary = Path(directory)
                        self.require_space(temporary)
                        stub = temporary / 'incompatible.c'
                        stub.write_text('#include "swoole_rs.h"\n'
                                        f'uint32_t swr_v1_abi_version(void) {{ return {abi}; }}\n'
                                        f'uint64_t swr_v1_feature_bits(void) {{ return {bits}; }}\n')
                        obj, archive = temporary / 'incompatible.o', temporary / 'libswoole_rs_php.a'
                        run(shlex.split(os.environ.get('CC', 'cc')) + ['-fPIC', '-I', str(ROOT / 'native/include'),
                            '-c', str(stub), '-o', str(obj)])
                        # Preserve real module initialization/error paths: incomplete
                        # function stubs can crash before startup reaches its ABI guard.
                        copy_private(prepared, archive)
                        run(['ar', 'rcs', str(archive), str(obj)])
                        # Absolute .lo paths preserve each libtool object's .libs lookup. Move
                        # both outputs and libtool's working directory away from the build.
                        relink = command.copy()
                        relink[archive_index] = str(archive)
                        relink[output_index] = str(temporary / 'swoole.la')
                        relink[rpath_index] = str(temporary)
                        for index in stats_indices:
                            relink[index] = 'CCACHE_STATSLOG=' + str(temporary / 'link.ccache.log')
                        run(relink, cwd=temporary)
                        extension = temporary / '.libs/swoole.so'
                        self.assertTrue(extension.is_file(), 'Relink did not produce a temporary PHP extension')
                        result = self.php(extension, '-r', 'echo "startup-accepted\\n";')
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertIn(message, result.stdout + result.stderr)
                        self.assertNotIn('startup-accepted', result.stdout)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='Load before swoole, e.g. sockets')
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--reference', type=Path, default=ROOT / 'target/upstream/src/modules/swoole.so')
    parser.add_argument('--hybrid', type=Path, default=ROOT / 'target/hybrid/rust/modules/swoole.so')
    parser.add_argument('--archive', type=Path, help='Defaults to the selected hybrid build’s copied archive')
    parser.add_argument('--feature-bits', type=int, help='Defaults to the selected hybrid build manifest')
    parser.add_argument('--startup-only', action='store_true', help='Positive startup/flag gates; full incompatible-archive relinks remain integration-only')
    ARGS = parser.parse_args()
    build = ARGS.hybrid.resolve().parents[2]
    if ARGS.archive is None:
        ARGS.archive = build / 'rust-prefix/lib/libswoole_rs_ffi.a'
    if ARGS.feature_bits is None:
        manifest = build / 'build-manifest.json'
        if not manifest.is_file():
            parser.error(f'Missing build manifest: {manifest}; provide --feature-bits explicitly')
        ARGS.feature_bits = json.loads(manifest.read_text())['migrated_feature_bits']
    for path in [ARGS.reference, ARGS.hybrid, ARGS.archive]:
        if not path.is_file():
            parser.error(f'Missing built artifact: {path}; build reference and hybrid first')
    tests = ['BridgeTests.test_reference_has_no_bridge_marker', 'BridgeTests.test_hybrid_markers_and_phpinfo',
             'BridgeTests.test_selected_abi_exports', 'BridgeTests.test_public_http_status_array_export',
             'BridgeTests.test_module_information_exports_follow_profile'] if ARGS.startup_only else []
    unittest.main(argv=[sys.argv[0], *tests], verbosity=2)
