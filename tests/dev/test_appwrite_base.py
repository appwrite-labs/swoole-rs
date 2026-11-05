"""Only the Swoole stage of appwrite/docker-base changes, and it keeps Appwrite's configure flags."""
import unittest

from tools.appwrite_base import rewrite

DOCKERFILE = '''FROM php AS compile
RUN apk add git

FROM compile AS swoole
RUN \\\\
  git fetch swoole && \\\\
  ./configure \\\\
    --enable-brotli \\\\
    --enable-sockets \\\\
    --with-openssl-dir=/usr && \\\\
  make && make install && \\\\
  strip /artifacts/swoole.so

FROM compile AS final
COPY --from=swoole /artifacts/ /tmp/exts/
'''


class AppwriteBase(unittest.TestCase):
    def test_only_the_swoole_stage_changes(self):
        for variant in ['hybrid', 'upstream']:
            out = rewrite(DOCKERFILE, variant)
            self.assertTrue(out.startswith('FROM php AS compile\nRUN apk add git\n\nFROM compile AS swoole\n'))
            self.assertTrue(out.endswith('FROM compile AS final\nCOPY --from=swoole /artifacts/ /tmp/exts/\n'))
            self.assertIn('--configure=--enable-brotli --configure=--with-openssl-dir=/usr', out)
            self.assertNotIn('--configure=--enable-sockets', out)  # --php-sockets covers it
            self.assertEqual('build_hybrid.py' in out, variant == 'hybrid')

    def test_unknown_shape_is_refused(self):
        with self.assertRaises(ValueError):
            rewrite('FROM php AS compile\n', 'hybrid')


if __name__ == '__main__':
    unittest.main()
