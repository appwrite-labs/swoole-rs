#!/usr/bin/env python3
"""The actual ABI export gate must reject missing newer-version ABI exports."""
import importlib.util
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('bridge_gate', Path(__file__).with_name('test_bridge.py'))
bridge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bridge)


class SelectedExports(unittest.TestCase):
    def check(self, symbols, conditional=False, config=''):
        with tempfile.TemporaryDirectory() as folder:
            build = Path(folder)
            (build / 'config.h').write_text(config)
            header = build / 'native/http/swoole_rs_http.h'
            header.parent.mkdir(parents=True)
            content = b'''uint32_t swr_v1_http_plan(void);
void *swr_v2_http_directory_new(void);
/* void swr_v99_comment(void); */
// void swr_v88_comment(void);
'''
            if conditional:
                content += b"\n#ifdef __linux__\nvoid swr_v3_linux_backend(void);\n#else\nvoid swr_v3_other_backend(void);\n#endif\n"
            content += b"#ifdef SW_THREAD\nvoid swr_v1_phplocks_thread_atomic_minit(void);\n#endif\n"
            header.write_bytes(content)
            real_run = bridge.run
            def command(command, **kwargs):
                if command[0] == 'nm':
                    return SimpleNamespace(stdout=symbols)
                if command[0] == 'test-php-config':
                    return SimpleNamespace(stdout='')
                return real_run(command, **kwargs)
            with patch.object(bridge, 'ARGS', SimpleNamespace(extension=build / 'swoole.so', source=build / 'src',
                    php_config='test-php-config'), create=True), \
                 patch.object(bridge, 'headers_for', return_value=[header]), \
                 patch.object(bridge, 'run', side_effect=command):
                bridge.BridgeTests('test_selected_abi_exports').test_selected_abi_exports()

    def test_all_declared_versions_required(self):
        with self.assertRaisesRegex(AssertionError, 'swr_v2_http_directory_new'):
            self.check('0000 T swr_v1_http_plan\n')

    def test_both_versions_present(self):
        self.check('0000 T swr_v1_http_plan\n0001 T swr_v2_http_directory_new\n')

    def test_only_target_backend_is_required(self):
        target = 'linux' if bridge.sys.platform.startswith('linux') else 'other'
        self.check('T swr_v1_http_plan\nT swr_v2_http_directory_new\nT swr_v3_' + target + '_backend\n', True)

    def test_missing_target_backend_is_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'swr_v3_'):
            self.check('T swr_v1_http_plan\nT swr_v2_http_directory_new\n', True)

    def test_configure_gated_exports_follow_the_builds_config_h(self):
        exports = '0000 T swr_v1_http_plan\n0001 T swr_v2_http_directory_new\n'
        self.check(exports)
        with self.assertRaisesRegex(AssertionError, 'swr_v1_phplocks_thread_atomic_minit'):
            self.check(exports, config='#define SW_THREAD 1\n')
        self.check(exports + 'T swr_v1_phplocks_thread_atomic_minit\n', config='#define SW_THREAD 1\n')

    def test_apple_symbol_prefix(self):
        self.check('0000 T _swr_v1_http_plan\n0001 T _swr_v2_http_directory_new\n')


if __name__ == '__main__':
    unittest.main(verbosity=2)
