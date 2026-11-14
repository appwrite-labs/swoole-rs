"""Observed identities cannot disguise a changed binary or a Rust reference."""
from pathlib import Path
import hashlib
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from tools import extension_identity


class ExtensionIdentity(unittest.TestCase):
    def test_reference_receipt_rejects_mutation_and_feature_misselection(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)/'swoole.so';path.write_bytes(b'original')
            metadata = {'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'rust_feature_bits':0}
            engine = SimpleNamespace(extension=path,metadata=lambda:metadata)
            with patch.object(extension_identity.subprocess,'check_output',return_value=extension_identity.COMMIT):
                receipt = extension_identity.observe(engine,Path(directory))
                self.assertFalse(receipt['build_attestation'])
                metadata['rust_feature_bits']=32768
                with self.assertRaisesRegex(ValueError,'Rust features'):
                    extension_identity.observe(engine,Path(directory))
                metadata['rust_feature_bits']=0
                def changed():
                    path.write_bytes(b'changed');return metadata
                engine.metadata=changed
                with self.assertRaisesRegex(ValueError,'changed'):
                    extension_identity.observe(engine,Path(directory))
            with patch.object(extension_identity.subprocess,'check_output',return_value='unrelated'):
                with self.assertRaisesRegex(ValueError,'pinned'):
                    extension_identity.observe(engine,Path(directory))


if __name__ == '__main__':
    unittest.main()
