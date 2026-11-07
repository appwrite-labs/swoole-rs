"""Configured SDK flags are literal argv; Makefile commands never execute."""
import tempfile
import unittest
from pathlib import Path
from count import configured_flags


class ConfiguredFlags(unittest.TestCase):
    def flags(self, text):
        with tempfile.TemporaryDirectory() as directory:
            core=Path(directory)
            if text is not None:(core/'Makefile').write_text(text)
            return configured_flags(core)

    def test_configured_paths_and_continuations(self):
        flags,record=self.flags('top_builddir = /sdk\nINCLUDES = -I. -I$(top_builddir)/include \\\n -I"/sdk with spaces/include"\nINCLUDES += -Ithirdparty\nCPPFLAGS ?= -DHAVE_CONFIG_H\nCPPFLAGS ?= -DWRONG\nCPPFLAGS += -DVALUE=1 # configured comment\n')
        self.assertEqual(flags,['-I.','-I/sdk/include','-I/sdk with spaces/include','-Ithirdparty','-DHAVE_CONFIG_H','-DVALUE=1'])
        self.assertEqual(record['argv'],flags)
        self.assertEqual(len(record['sha256']),64)

    def test_absent_makefile_preserves_explicit_sdk_mode(self):
        self.assertEqual(self.flags(None)[0],[])

    def test_nonliteral_and_recursive_expressions_fail(self):
        for text in ['INCLUDES = $(shell touch forbidden)\n','CPPFLAGS = `touch forbidden`\n','INCLUDES = $(unknown)\n','A = $(B)\nB = ${A}\nINCLUDES = $(A)\n']:
            with self.subTest(text=text),self.assertRaises(ValueError):self.flags(text)


if __name__=='__main__':unittest.main()
