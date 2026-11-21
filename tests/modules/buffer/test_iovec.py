"""Positive offset-0 nonempty POSIX iovec vectors, native versus the Buffer patches with Rust.

Rust-only tests separately exercise validation, the model and rollback.
"""
from native import native_and_selected
from tools.harness import ROOT


def test_iovec(pinned, archive, pytestconfig):
    tree = pinned('include', 'src/core/buffer.cc')
    native, selected = native_and_selected(tree, ROOT / 'tests/modules/buffer/iovec.cc', archive,
                                           pytestconfig.getoption('sanitize')).values()
    assert native['cases'] == selected['cases'] == 12
    assert selected['layout'] == native['layout']
    assert selected['consumer_checks'] == native['consumer_checks'] + 4
