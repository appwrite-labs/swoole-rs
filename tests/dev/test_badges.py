"""tools/badges.py counts upstream lines the patch set deletes (net), passing PHPTs and speed."""
from pathlib import Path
import subprocess
import os
import random
from unittest.mock import patch
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import badges  # noqa: E402


def diff(path, old, new, context):
    """A one-hunk git patch replacing `old` lines with `new` lines at the top of `path`,
    followed by one unchanged `context` line (git apply needs context)."""
    body = ''.join(f'-{line}\n' for line in old) + ''.join(f'+{line}\n' for line in new) + f' {context}\n'
    return (f'diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n'
            f'@@ -1,{len(old) + 1} +1,{len(new) + 1} @@\n{body}')


class Badges(unittest.TestCase):
    def test_net_upstream_deletions_over_first_party_lines(self):
        with tempfile.TemporaryDirectory() as directory:
            source, patches = Path(directory) / 'src-tree', Path(directory) / 'patches'
            (source / 'src').mkdir(parents=True)
            (source / 'ext-src').mkdir()
            (source / 'thirdparty').mkdir()
            patches.mkdir()
            (source / 'src/a.cc').write_text(''.join(f'line {n}\n' for n in range(10)))
            (source / 'ext-src/php_swoole_library.h').write_text('x\n' * 1000)  # not counted
            (source / 'thirdparty/lib.c').write_text('x\n' * 1000)  # not counted
            subprocess.run(['git', 'init', '-q', str(source)], check=True)
            # The first patch replaces 3 upstream lines with 2 lines of glue; the second
            # removes that glue again. Only the 3 upstream lines count as deleted.
            first = patches / '0001.patch'
            first.write_text(diff('src/a.cc', ['line 0', 'line 1', 'line 2'], ['glue 1', 'glue 2'], 'line 3'))
            second = patches / '0002.patch'
            second.write_text(diff('src/a.cc', ['glue 1', 'glue 2'], [], 'line 3'))
            rows = [{'path': str(first), 'upstream_paths': ['src/a.cc'], 'sha256': None},
                    {'path': str(second), 'upstream_paths': ['src/a.cc'], 'sha256': None}]
            self.assertEqual(badges.replaced(badges.kept(source, rows)), (3, 10))

    def test_upstream_lines_moved_to_another_file_are_not_deleted(self):
        moving = ['int helper(int value) {', '    return value * 2;', '}']
        with tempfile.TemporaryDirectory() as directory:
            source, patch_path = Path(directory) / 'source', Path(directory) / 'move.patch'
            (source / 'src').mkdir(parents=True)
            (source / 'src/a.cc').write_text('\n'.join([*moving, 'int a_stays;']) + '\n')
            (source / 'src/b.cc').write_text('int b_stays;\n')
            patch_path.write_text(diff('src/a.cc', moving, [], 'int a_stays;')
                                  + diff('src/b.cc', [], moving, 'int b_stays;'))
            rows = [{'path': str(patch_path), 'upstream_paths': ['src/a.cc', 'src/b.cc'], 'sha256': None}]
            self.assertEqual(badges.replaced(badges.kept(source, rows)), (0, 5))

    def test_added_glue_cannot_invent_original_deletions_in_repeated_lines(self):
        # A small alphabet models repeated braces/branches in a rewritten TU.
        # Default diff heuristics report 999 -> 1001 removals after this pure
        # addition. Minimal matching correctly reports 930 -> 927 instead.
        generator = random.Random(1967)
        for _ in range(3):
            original = [str(generator.randrange(12)) for _ in range(1800)]
            candidate = [str(generator.randrange(12)) for _ in range(2200)]
        glue = ['0', '1', '2', '3', '4', '5', '0', '1', '2', '3', '4', '5', '0']
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'source'
            (source / 'src').mkdir(parents=True)
            (source / 'src/a.cc').write_text('\n'.join([*original, 'sentinel']) + '\n')
            first, second = Path(directory) / 'first.patch', Path(directory) / 'second.patch'
            first.write_text(diff('src/a.cc', original, candidate, 'sentinel'))
            second.write_text(diff('src/a.cc', [], glue, candidate[0]))
            rows = [{'path': str(path), 'upstream_paths': ['src/a.cc'], 'sha256': None}
                    for path in [first, second]]
            for algorithm in ['myers', 'histogram']:
                # Host preferences must not change the published measurement.
                environment = {'GIT_CONFIG_COUNT': '1', 'GIT_CONFIG_KEY_0': 'diff.algorithm',
                               'GIT_CONFIG_VALUE_0': algorithm}
                with self.subTest(algorithm=algorithm), patch.dict(os.environ, environment):
                    self.assertEqual(badges.replaced(badges.kept(source, rows[:1])), (930, 1801))
                    self.assertEqual(badges.replaced(badges.kept(source, rows)), (927, 1801))

    def test_phpt_counts_cargo_passes_over_every_pinned_test(self):
        rows = [{'test': 'a', 'engine': 'upstream', 'status': 'passed'}, {'test': 'a', 'engine': 'cargo', 'status': 'passed'},
                {'test': 'b', 'engine': 'upstream', 'status': 'passed'}, {'test': 'b', 'engine': 'cargo', 'status': 'failed'},
                {'test': 'c', 'engine': 'upstream', 'status': 'skipped'}, {'test': 'c', 'engine': 'cargo', 'status': 'skipped'},
                {'test': 'd', 'engine': 'upstream', 'status': 'passed'}, {'test': 'd', 'engine': 'cargo', 'status': 'failed', 'rerun': 'passed'}]
        self.assertEqual(badges.phpt_passing(rows), (2, 4))
        # Another job's gate (a build variant) passes b; c can't apply here.
        rows.append({'test': 'b', 'engine': 'cargo', 'status': 'passed'})
        self.assertEqual(badges.phpt_passing(rows, {'c': 'darwin only'}), (3, 3))

    def test_speed_counts_workloads_faster_than_upstream_and_names_the_slowest(self):
        result = {'requests': 10, 'instructions_per_request': {
            'plaintext': {'hybrid': 103.0, 'upstream': 100.0}, 'timer': {'hybrid': 80.0, 'upstream': 100.0},
            'websocket': {'hybrid': 120.0, 'upstream': 100.0}}}
        badge, table = badges.speed(result, {'plaintext': 1.05, 'websocket': 1.1}, 'abcdef0123')
        self.assertEqual(badge['message'], '1/3 faster · slowest websocket 1.20×')
        self.assertEqual(badge['color'], 'orange')
        self.assertIn('| websocket | 120 | 100 | 1.2000× | 1.1× | ❌ |', table)
        self.assertIn('| timer | 80 | 100 | 0.8000× |  | ✅ |', table)
        self.assertIn('`abcdef0`', table)


if __name__ == '__main__':
    unittest.main()
