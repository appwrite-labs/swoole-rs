"""Sanitizer comparisons retain libc's locales and detect invalid reads."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from tools.asan import ROOT, compilers


PROBE = r'''
#include <locale.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <strings.h>

int main(int argc, char **argv) {
    if (argc > 1) {
        char *short_string = (char *)malloc(1);
        short_string[0] = 'a';
        return argv[1][0] == 'n' ? strncasecmp(short_string, "aa", 2)
                                 : strcasecmp(short_string, "aa");
    }
    const char *locales[] = {"C", "C.UTF-8", "en_US.ISO8859-1",
                             "tr_TR.ISO8859-9", "tr_TR.UTF-8"};
    for (const char **locale = locales; locale != locales + 5; ++locale) {
        if (!setlocale(LC_ALL, *locale)) return 2;
        uint64_t hash = 0;
        for (unsigned a = 0; a < 256; ++a) {
            for (unsigned b = 0; b < 256; ++b) {
                char left[] = {(char)a, 'a', 0, 'B'};
                char right[] = {(char)b, 'A', 0, 'c'};
                for (size_t length = 0; length <= 4; ++length) {
                    int result = strncasecmp(left, right, length);
                    hash = hash * 31 + (result > 0) - (result < 0);
                }
                int result = strcasecmp(left, right);
                hash = hash * 31 + (result > 0) - (result < 0);
            }
        }
        printf("%s %llu\n", *locale, (unsigned long long)hash);
    }
}
'''


@unittest.skipUnless(sys.platform == 'linux' and shutil.which('clang'),
                     'requires the Linux sanitizer toolchain and gate locales')
class CaseComparisons(unittest.TestCase):
    def test_libc_equivalence_and_memory_checks(self):
        (ROOT / 'target').mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=ROOT / 'target', prefix='asan-case-') as directory:
            out = Path(directory)
            source = out / 'probe.c'
            source.write_text(PROBE)
            plain = out / 'plain'
            subprocess.run(['clang', '-fno-builtin', source, '-o', plain], check=True,
                           capture_output=True)
            expected = subprocess.check_output([plain], text=True)
            env = compilers(out / 'bin')
            env.update(ASAN_OPTIONS='', UBSAN_OPTIONS='', LSAN_OPTIONS='')
            for compiler in ['CC', 'CXX']:
                with self.subTest(compiler=compiler):
                    obj, binary = out / 'probe.o', out / 'probe'
                    subprocess.run([env[compiler], '-fno-builtin', '-c', source, '-o', obj],
                                   env=env, check=True, capture_output=True)
                    subprocess.run([env[compiler], obj, '-o', binary], env=env,
                                   check=True, capture_output=True)
                    self.assertEqual(subprocess.check_output([binary], env=env, text=True), expected)
                    for comparison in ['n', 'unbounded']:
                        failed = subprocess.run([binary, comparison], env=env, text=True,
                                                capture_output=True)
                        self.assertNotEqual(failed.returncode, 0)
                        self.assertIn('AddressSanitizer: heap-buffer-overflow', failed.stderr)


if __name__ == '__main__':
    unittest.main()
