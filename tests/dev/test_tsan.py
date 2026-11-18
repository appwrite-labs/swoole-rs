"""tools/tsan.py's LIMIT counts every report outside upstream's runs, exit_guard's included, and
their text reaches the artifact under the PHPT that wrote it."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from tools import tsan  # noqa: E402


class OutsideUpstream(unittest.TestCase):
    def test_upstream_runs_are_left_out(self):
        counts = {'upstream': 5, 'upstream-guard': 3, 'cargo': 2, 'cargo-guard': 1}
        self.assertEqual(tsan.outside_upstream(counts), 3)


RACE = """==================
WARNING: ThreadSanitizer: data race (pid=4242)
  Write of size 8 at 0x7b0400000010 by thread T1:
    #0 swoole_rs::thread::map::set crates/thread/src/map.rs:41 (cargo.so+0x1234)
SUMMARY: ThreadSanitizer: data race crates/thread/src/map.rs:41 in swoole_rs::thread::map::set
==================
"""


class Kept(unittest.TestCase):
    def test_a_cargo_report_keeps_its_text_and_test_and_upstream_stays_out(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reports = root / 'reports'
            # The PHP every run.py process starts, standing in for the instrumented one: it prints
            # where TSan would write.
            php = root / 'php'
            php.write_text('#!/bin/sh\necho "$TSAN_OPTIONS"\n')
            php.chmod(0o755)
            named = root / 'named-php'
            named.write_text(tsan.named(php, reports / 'cargo'))
            named.chmod(0o755)

            def log_path(*args, **env):
                return subprocess.run([named, *args], env=env, capture_output=True, text=True, check=True).stdout.strip()

            self.assertEqual(log_path('/s/tests/run-tests.php', '-p', '/t/php', '/s/tests/swoole_thread/map.phpt',
                                      TSAN_OPTIONS='log_path=/r/report'), 'log_path=/r/report')
            for section in ('map.php', 'map.skip.php', 'map.clean.php'):
                self.assertEqual(log_path('-d', 'auto_prepend_file=/s/prepend.php', '-f', f'/s/tests/swoole_thread/{section}',
                                          TSAN_OPTIONS='log_path=/r/report'),
                                 f'log_path=/r/report:log_path={reports}/cargo/swoole_thread-map')
            # A server the test starts is still that test's.
            self.assertTrue(log_path('/s/tests/include/server.php', TSAN_PHPT='swoole_thread-map', TSAN_OPTIONS='')
                            .endswith(f'{reports}/cargo/swoole_thread-map'))

            (reports / 'cargo').mkdir(parents=True)
            (reports / 'cargo/swoole_thread-map.4242').write_text(RACE)
            (reports / 'upstream').mkdir()
            (reports / 'upstream/swoole_thread-map.17').write_text('upstream race in swoole_thread.cc')
            output = root / 'reports.txt'
            tsan.kept(reports, output)
            text = output.read_text()
            self.assertIn('==== cargo: swoole_thread-map (swoole_thread-map.4242)', text)
            self.assertIn(RACE, text)
            self.assertNotIn('upstream', text)


if __name__ == '__main__':
    unittest.main()
