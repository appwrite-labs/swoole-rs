#!/usr/bin/env python3
"""Build swoole.so with Cargo alone (crates/ext): no phpize, configure or make.

crates/ext's build.rs compiles native/ and the bundled thirdparty C from the patched tree this
prepares in target/ext/src, then the extension is checked to load and report the pinned version.
Prints `Built cargo: <path>` (target/ext/swoole.so).
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_upstream import VERSION
from tools.harness import ROOT
from tools.patches import apply


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--profile', default='release', help='Cargo profile (default: release)')
    args = parser.parse_args()
    out = ROOT / 'target/ext'
    tree = out / 'src'
    apply(tree)
    env = {**os.environ, 'SWOOLE_SRC': str(tree), 'PHP_CONFIG': args.php_config}
    subprocess.run(['cargo', 'build', '--locked', '--profile', args.profile, '-p', 'swoole-rs-ext', '--features', 'php'],
                   cwd=ROOT, env=env, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    built = target / ('debug' if args.profile == 'dev' else args.profile) / (
        'libswoole.dylib' if sys.platform == 'darwin' else 'libswoole.so')
    extension = out / 'swoole.so'
    shutil.copy2(built, extension)
    php = Path(subprocess.check_output([args.php_config, '--php-binary'], text=True).strip())
    version = subprocess.run([str(php), '-n', '-d', f'extension={extension}', '-r', 'echo SWOOLE_VERSION;'],
                             capture_output=True, text=True)
    if version.stdout != VERSION.removeprefix('v'):
        raise SystemExit(f'{extension} does not load as Swoole {VERSION}:\n{version.stdout}{version.stderr}')
    print(f'Built cargo: {extension}')


if __name__ == '__main__':
    main()
