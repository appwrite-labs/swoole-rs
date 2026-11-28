"""A low shared volume blocks builds before subprocesses or source mutation."""
import os
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from tools.build_upstream import build_extension, require_build_space


class DiskSpace(unittest.TestCase):
    def test_low_disk_prevents_native_build_without_mutation(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary)
            with patch.dict(os.environ, {'SWOOLE_RS_MIN_FREE_BYTES': '4096'}), \
                 patch('tools.build_upstream.shutil.disk_usage', return_value=SimpleNamespace(free=4095)), \
                 patch('tools.build_upstream.subprocess.run') as run:
                with self.assertRaisesRegex(RuntimeError, 'Build disk space low'):
                    build_extension(path, path/'build.log', SimpleNamespace(jobs=1))
                run.assert_not_called()
                self.assertEqual(list(path.iterdir()), [])

    def test_dev_ledger_records_blocked_build_as_infrastructure_failure(self):
        from tools import dev
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(dev, 'ROOT', root), \
                 patch('sys.argv', ['dev.py', 'edit', '--module', 'locks']), \
                 patch.object(dev, 'require_build_space', side_effect=RuntimeError('disk low')), \
                 patch.object(dev, 'execute') as run:
                self.assertEqual(dev.main(), 1)
                run.assert_not_called()
            report = json.loads(next(root.rglob('loop.json')).read_text())
            self.assertEqual(report['status'], 'failed')
            self.assertEqual(report['blocked_phase'], 'base64-unit')  # graduated modules are always selected
            self.assertEqual(report['infrastructure_error'], 'disk low')
            self.assertEqual(report['phases'], [])

    def test_exact_threshold_and_explicit_override(self):
        with patch.dict(os.environ, {'SWOOLE_RS_MIN_FREE_BYTES': '4096'}), \
             patch('tools.build_upstream.shutil.disk_usage', return_value=SimpleNamespace(free=4096)):
            self.assertEqual(require_build_space(Path('.')), 4096)
        with patch.dict(os.environ, {'SWOOLE_RS_MIN_FREE_BYTES': '-1'}):
            with self.assertRaises(ValueError):
                require_build_space(Path('.'))


if __name__ == '__main__':
    unittest.main()
