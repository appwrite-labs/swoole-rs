import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest
from corpus.upstream.tree import inventory


class OriginalTree(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.source = Path(self.temporary.name)
        self.git('init', '-q')
        for name, body in [('tests/example/a.phpt', b'--TEST--\na\n--FILE--\nx\n--EXPECT--\ny\n'),
                           ('tests/example/nested/b.phpt', b'--TEST--\nb\n'),
                           ('tests/example/nested/input.dat', b'\x00\xff'),
                           ('tests/include/bootstrap.php', b'<?php // original ?>'),
                           ('tests/other/ignored.phpt', b'other')]:
            p = self.source / name
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(body)
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        self.pin = self.git('rev-parse', 'HEAD').decode().strip()

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.source), *args])

    def test_every_original_nested_test_and_binary_support_are_exact(self):
        report = inventory(self.source, 'example', 'example', ['bootstrap', 'bootstrap'], pin=self.pin)
        self.assertEqual([r['path'] for r in report['tests']], ['tests/example/a.phpt', 'tests/example/nested/b.phpt'])
        self.assertEqual(report['tests'][0]['requires'], ['bootstrap'])
        support = {r['path']: r['sha256'] for r in report['support_files']}
        self.assertEqual(support['tests/example/nested/input.dat'], hashlib.sha256(b'\x00\xff').hexdigest())
        self.assertIn('tests/include/bootstrap.php', support)
        self.assertEqual(report['inventory']['cases'], 2)

    def test_dirty_originals_and_wrong_pin_are_rejected(self):
        (self.source / 'tests/example/a.phpt').write_text('changed')
        with self.assertRaisesRegex(ValueError, 'tracked changes'):
            inventory(self.source, 'example', 'example', [], pin=self.pin)
        with self.assertRaisesRegex(ValueError, 'HEAD differs'):
            inventory(self.source, 'example', 'example', [], pin='wrong')

    def test_empty_and_traversal_groups_are_rejected(self):
        for group in ['../example', '/example', 'example/../other', 'example;echo']:
            with self.assertRaises(ValueError):
                inventory(self.source, group, 'example', [], pin=self.pin)
        with self.assertRaisesRegex(ValueError, 'empty'):
            inventory(self.source, 'missing', 'example', [], pin=self.pin)
