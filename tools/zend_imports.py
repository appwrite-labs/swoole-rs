#!/usr/bin/env python3
"""Require a reviewed bailout reason for PHP imports outside crates/zend*.

This is a declaration inventory, not a proof about call sites. All cfg branches
are scanned, including link_name aliases. Keep the exact path/symbol allowlist
in zend_imports.json current when moving or deleting declarations.
"""
import json
from pathlib import Path
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.externs import ROOT, block, strip

PHP = re.compile(r'^(?:_?(?:zend|php|zval|tsrm|sapi)_|swr_(?:zend|zval|bytes|php\w*)_|(?:executor|compiler|core|sapi)_globals$)')
# Zend entry points without its namespace prefix.
PHP_NAMES = {'_emalloc', '_ecalloc', '_erealloc', '_efree', '_estrdup', '_estrndup',
             '_safe_emalloc', '_safe_erealloc', 'ts_resource_ex', 'object_init_ex',
             'object_properties_init', 'instanceof_function', 'display_ini_entries',
             '_call_user_function_impl', 'call_user_function', 'call_user_function_ex'}
ITEM = re.compile(r'((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?'
                  r'(?:safe\s+|unsafe\s+)?(?:fn\s+|static\s+(?:mut\s+)?)(?:r#)?(\w+)(?=\s*(?:[(<:]))')


def imports(text, shims=()):
    """PHP linker symbols in foreign blocks, never Rust definitions or fn pointers."""
    text = strip(text)
    for ext in re.finditer(r'\bextern(?:\s+"[^"]+")?\s*\{', text):
        body = block(text, ext.end())
        for item in ITEM.finditer(body):
            attrs, name = item.groups()
            aliases = re.findall(r'\blink_name\s*=\s*"([^"]+)"', attrs)
            # An unconditional link_name replaces the Rust name. cfg_attr may
            # leave that name in use, so inventory both possibilities.
            direct = re.search(r'#\[\s*link_name\s*=', attrs)
            for symbol in set(aliases + ([] if direct else [name])):
                if PHP.match(symbol) or symbol in PHP_NAMES or symbol in shims:
                    yield symbol


def inventory(root):
    # Include shims whose names predate swr_zend_ (swr_zval_copy, swr_bytes_*, ...).
    shims = {name for p in (root / 'crates/zend-sys/src').glob('*.[ch]')
             for name in re.findall(r'\b(swr_\w+)\s*\(', strip(p.read_text()))}
    return {f'{p.relative_to(root)}:{symbol}'
            for p in (root / 'crates').glob('*/src/**/*.rs')
            if not p.relative_to(root / 'crates').parts[0].startswith('zend')
            for symbol in imports(p.read_text(), shims)}


def problems(found, allowed):
    return ([f'{key}: missing bailout justification in tools/zend_imports.json'
             for key in sorted(found - allowed.keys())]
            + [f'{key}: stale import justification; remove it'
               for key in sorted(allowed.keys() - found)]
            + [f'{key}: empty bailout justification'
               for key, reason in sorted(allowed.items())
               if not isinstance(reason, str) or not reason.strip()])


def main(root=ROOT):
    found = inventory(root)
    allowed = json.loads((root / 'tools/zend_imports.json').read_text())
    errors = problems(found, allowed)
    for error in errors:
        print(error, file=sys.stderr)
    print(f'Zend imports: {len(found)} declarations, {len(errors)} problems')
    return bool(errors)


if __name__ == '__main__':
    sys.exit(main())
