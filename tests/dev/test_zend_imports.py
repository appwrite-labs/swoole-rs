"""New PHP entry points need review even when hidden behind cfg or a Rust alias."""
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest

from tools import zend_imports as guard


class ZendImports(unittest.TestCase):
    def test_linker_names_and_every_configuration(self):
        source = '''
        #[cfg(target_os = "windows")]
        unsafe extern "C" {
            #[cfg(feature = "optional")]
            #[link_name = "zend_call_function"]
            pub(crate) safe fn invoke();
            #[cfg_attr(windows, link_name = "php_windows_call")]
            fn php_other_call();
            static mut executor_globals: Globals;
            fn swr_zend_observe(observer: &'static Observer);
            fn read(fd: i32);
        }
        unsafe extern "C-unwind" { fn _efree(ptr: *mut u8); }
        unsafe extern { fn r#zend_default_abi(); }
        unsafe extern "system" { fn sapi_header_op(); }
        '''
        self.assertEqual(set(guard.imports(source)), {
            'zend_call_function', 'php_windows_call', 'php_other_call',
            'executor_globals', 'swr_zend_observe', '_efree',
            'zend_default_abi', 'sapi_header_op',
        })

    def test_ignores_definitions_comments_and_callback_types(self):
        self.assertEqual(set(guard.imports('''
        // unsafe extern "C" { fn zend_commented(); }
        /* unsafe extern "C" { fn php_commented(); } */
        extern "C" fn php_export() {}
        type Handler = extern "C" fn();
        unsafe extern "C" {
            fn ordinary(callback: extern "C" fn(), value: &'static swr_zend_value);
        }
        ''')), set())

    def test_php_name_renamed_to_non_php_symbol_is_not_an_import(self):
        self.assertEqual(set(guard.imports('''unsafe extern "C" {
            #[link_name = "native_helper"] fn zend_local_name();
        }''')), set())

    def test_shims_without_php_namespace(self):
        self.assertEqual(set(guard.imports('''unsafe extern "C" {
            fn old_helper(); fn unrelated();
        }''', {'old_helper'})), {'old_helper'})

    def test_missing_moved_stale_and_empty_justifications_fail(self):
        found = {'a.rs:zend_existing', 'b.rs:zend_new'}
        self.assertEqual(guard.problems(found, {
            'a.rs:zend_existing': 'Direct executor read.',
            'b.rs:zend_new': 'Inside contain_with in caller.',
        }), [])
        errors = guard.problems(found, {'a.rs:zend_existing': ' ', 'old.rs:zend_new': 'Moved.'})
        self.assertEqual(len(errors), 3)
        self.assertTrue(any('b.rs:zend_new: missing' in error for error in errors))
        self.assertTrue(any('old.rs:zend_new: stale' in error for error in errors))
        self.assertTrue(any('a.rs:zend_existing: empty' in error for error in errors))

    def test_new_import_fails_the_command_and_removal_requires_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'crates/family/src/lib.rs'
            source.parent.mkdir(parents=True)
            source.write_text('unsafe extern "C" { fn zend_existing(); }')
            shims = root / 'crates/zend-sys/src'
            shims.mkdir(parents=True)
            (shims / 'shim.c').write_text('void swr_legacy(void) {}')
            (shims / 'lib.rs').write_text('unsafe extern "C" { fn zend_exempt(); }')
            allowed = root / 'tools/zend_imports.json'
            allowed.parent.mkdir()
            allowed.write_text(json.dumps({'crates/family/src/lib.rs:zend_existing': 'Executor read.'}))
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(guard.main(root), 0)
                source.write_text('unsafe extern "C" { fn zend_existing(); fn swr_legacy(); }')
                self.assertEqual(guard.main(root), 1)
                source.write_text('')
                self.assertEqual(guard.main(root), 1)


if __name__ == '__main__':
    unittest.main()
