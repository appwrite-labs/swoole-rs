"""Run Clippy's buffer growth restrictions on production protocol libraries."""

import os
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CRATES = ('brotli', 'clients', 'deflate', 'framing', 'http', 'http2', 'inflate', 'protocols', 'websocket', 'zlib')
METHODS = (
    'std::vec::Vec::push', 'std::iter::Extend::extend',
    'std::vec::Vec::extend_from_slice', 'std::vec::Vec::extend_from_within',
    'std::vec::Vec::reserve', 'std::vec::Vec::reserve_exact',
    'std::vec::Vec::try_reserve', 'std::vec::Vec::try_reserve_exact',
    'std::vec::Vec::resize', 'std::vec::Vec::resize_with',
)


def main():
    # Clippy takes one configuration directory per invocation. Preserve the
    # workspace rules and add these methods only for this group of libraries.
    config = ROOT / 'target' / 'protocol-buffer-lint'
    config.mkdir(parents=True, exist_ok=True)
    rules = (ROOT / 'clippy.toml').read_text()
    start = rules.index('disallowed-methods = [') + len('disallowed-methods = [')
    methods = ''.join(f'\n  {{ path = "{path}", reason = "use bounded storage for peer input" }},'
                      for path in METHODS)
    path = config / 'clippy.toml'
    contents = rules[:start] + methods + rules[start:]
    if not path.exists() or path.read_text() != contents:
        path.write_text(contents)
    packages = [arg for name in CRATES for arg in ('-p', f'swoole-rs-{name}')]
    return subprocess.run(
        ['cargo', 'clippy', '--locked', '--lib', '--all-features', *packages,
         '--', '--no-deps', '--cfg', 'protocol_buffer_lint', '-D', 'warnings'],
        cwd=ROOT, env={**os.environ, 'CLIPPY_CONF_DIR': str(config)},
    ).returncode


if __name__ == '__main__':
    raise SystemExit(main())
