"""Argument-budget coverage for aliased registrations, delegation and untyped reads."""
from pathlib import Path
import tempfile
import unittest

from tools import budget
from tools.audit_php_args import audit


class PhpArgs(unittest.TestCase):
    def test_shared_handler_is_checked_against_each_registered_signature(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stubs = root / 'crates/phpstubs/src'
            handlers = root / 'crates/phpdemo/src'
            stubs.mkdir(parents=True)
            handlers.mkdir(parents=True)
            (stubs / 'demo.rs').write_text('''
pub static arginfo_strict: a::ArgInfo<2> = a::ArgInfo::new([
    a::begin(1, 0), a::arg_type(c"values", 0, false, z::IS_ARRAY, false, None),
]);
pub use self::arginfo_strict as arginfo_alias;
pub static arginfo_mixed: a::ArgInfo<2> = a::ArgInfo::new([
    a::begin(1, 0), a::arg_type(c"values", 0, false, z::IS_MIXED, false, None),
]);
''')
            (handlers / 'lib.rs').write_text('''
method_table! {
    c"strict" => zif_demo, &d::arginfo_alias, 0;
    c"mixed" => zif_demo, &d::arginfo_mixed, 0;
    c"foreign" => zif_foreign, &d::arginfo_strict, 0;
}
handlers! { zif_demo => run; }
fn run(call: &Call) { check(call, 1); }
fn check(frame: &Call, n: u32) {
    frame.value(n).and_then(Value::array);
    frame.array_ref(1);
    frame.value(1);
}
''')
            covered, registered, findings = audit(root)
            self.assertEqual(registered - covered, {('zif_foreign', 'arginfo_strict')})
            self.assertEqual([(f[0], f[4]) for f in findings],
                             [('arginfo_alias', 'value().and_then(Value::array)')])
            self.assertEqual({f[4] for f in audit(root, raw=True)[2]},
                             {'value', 'value().and_then(Value::array)'})

            (handlers.parent / 'Cargo.toml').touch()
            current = budget.counts(root)
            self.assertEqual(current['php_args'], {'phpdemo': 1})
            source = handlers / 'lib.rs'
            source.write_text(source.read_text().replace('value(n).and_then(Value::array)', 'array_ref(n)'))
            base = budget.counts(root)
            self.assertEqual(base['php_args'], {})
            self.assertEqual(set(budget.failures(current, base, root=root)), {'php_args'})
            self.assertFalse(budget.failures(base, current, root=root))
