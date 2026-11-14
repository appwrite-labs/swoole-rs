"""Real small Git checkouts exercise mutation during an integration phase."""
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import dev


class FrozenSource(unittest.TestCase):
    def test_clean_and_mutated_real_checkouts_preserve_outcomes(self):
        for mutation in ['none', 'before', 'tracked', 'untracked', 'head']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                def git(*args):
                    return subprocess.check_output(['git', '-c', 'core.hooksPath=/dev/null',
                        '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                        *args], cwd=root, text=True).strip()
                git('init', '--quiet')
                (root/'.gitignore').write_text('target/\n')
                (root/'value').write_text('original')
                git('add', '.gitignore', 'value'); git('commit', '--quiet', '-m', 'fixture')
                source = git('rev-parse', 'HEAD')
                git('commit', '--quiet', '--allow-empty', '-m', 'other revision')
                other = git('rev-parse', 'HEAD'); git('reset', '--quiet', '--hard', source)
                if mutation == 'before':
                    (root/'value').write_text('changed before launch')
                code = {
                    'tracked': "from pathlib import Path; Path('value').write_text('changed during phase')",
                    'untracked': "from pathlib import Path; Path('new-source').write_text('new input')",
                    'head': "import subprocess; subprocess.run(['git','update-ref','HEAD'," + repr(other) + "],check=True)",
                }.get(mutation, "from pathlib import Path; Path('target/ignored').write_text('scratch')")
                planned = [('positive', [sys.executable, '-c', code]),
                           ('later', [sys.executable, '-c', 'pass'])]
                with patch.object(dev, 'ROOT', root), patch.object(dev, 'plan', return_value=planned), \
                     patch('sys.argv', ['dev.py', 'integrate', '--module', 'locks', '--frozen-source', source]):
                    self.assertEqual(dev.main(), int(mutation != 'none'))
                report = json.loads(next(root.glob('target/reports/dev/*/loop.json')).read_text())
                self.assertEqual(report['frozen_source_required'], source)
                self.assertEqual(report['full_plan_passed'], mutation == 'none')
                if mutation == 'none':
                    self.assertEqual(report['status'], 'passed')
                    self.assertEqual(len(report['phases']), 2)
                else:
                    self.assertEqual(report['status'], 'failed')
                    self.assertEqual(report['blocked_phase'], 'positive')
                    self.assertIn('Frozen source', report['infrastructure_error'])
                    self.assertTrue(all(row['status'] == 'not_run' for row in report['not_run']))
                    if mutation == 'before':
                        self.assertEqual(report['phases'], [])
                        self.assertEqual(len(report['not_run']), 2)
                    else:
                        self.assertEqual(len(report['phases']), 1)
                        self.assertEqual(report['phases'][0]['returncode'], 0)
                        self.assertTrue(report['phases'][0]['failed'])
                        self.assertIn('source_error', report['phases'][0])
                        self.assertEqual([row['phase'] for row in report['not_run']], ['later'])

    def test_only_literal_commit_and_integration_are_accepted(self):
        for lane, source in [('candidate', 'a'*40), ('integrate', 'HEAD'), ('integrate', 'A'*40)]:
            with patch('sys.argv', ['dev.py', lane, '--frozen-source', source, '--plan']):
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    dev.main()
                self.assertEqual(error.exception.code, 2)


if __name__ == '__main__':
    unittest.main()
