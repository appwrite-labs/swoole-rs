#!/usr/bin/env python3
"""Build swoole.so with Cargo alone (crates/ext): no phpize, configure or make.

crates/ext's build.rs compiles native/ and the bundled thirdparty C from the patched tree this
prepares in target/ext/src, then the extension is checked to load and report the pinned version.
Prints `Built cargo: <path>` (target/ext/swoole.so) and writes target/ext/build-manifest.json,
which tools/ext_identity.py checks gates' engines against. `--php-sockets`, `--stdext`, `--fix-halfclose`
and build_hybrid's `--configure` switches turn on the crates/ext feature of the same build profile.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_hybrid import HOOK_SWITCHES
from tools.build_upstream import COMMIT, VERSION
from tools.harness import ROOT
from tools.patches import apply


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--profile', default='release', help='Cargo profile (default: release)')
    parser.add_argument('--php-sockets', action='store_true', help='Build with PHP sockets integration')
    parser.add_argument('--stdext', action='store_true', help='Build with the experimental stdext')
    parser.add_argument('--fix-halfclose', action='store_true', help='Build the opt-in half-close repairs')
    parser.add_argument('--configure', action='append', default=[], metavar='ARG',
                        help="A service hook's configure switch, as build_hybrid.py takes it; repeatable")
    args = parser.parse_args()
    out = ROOT / 'target/ext'
    tree = out / 'src'
    apply(tree)
    env = {**os.environ, 'SWOOLE_SRC': str(tree), 'PHP_CONFIG': args.php_config}
    switches = {**HOOK_SWITCHES, 'mysqlnd': '--enable-mysqlnd', 'uring-socket': '--enable-uring-socket',
                'cares': '--enable-cares', 'thread-context': '--enable-thread-context',
                'thread': '--enable-swoole-thread', 'stdext': '--enable-swoole-stdext'}
    features = ['php', *(['sockets'] if args.php_sockets else []), *(['stdext'] if args.stdext else []),
                *(['halfclose'] if args.fix_halfclose else [])]
    for flag in args.configure:
        name, _, value = flag.partition('=')
        feature = next((f for f, switch in switches.items() if switch == name), None)
        if not feature:
            raise SystemExit(f'build_ext.py: no crates/ext feature for {flag}')
        features.append(feature)
        # `unixODBC,DIR` and `instantclient,DIR` name the library's directory.
        if feature in ('odbc', 'oci') and ',' in value:
            env['SWOOLE_ODBC_DIR' if feature == 'odbc' else 'SWOOLE_ORACLE_DIR'] = value.split(',')[1]
    subprocess.run(['cargo', 'build', '--locked', '--profile', args.profile, '-p', 'swoole-rs-ext',
                    '--features', ','.join(features)], cwd=ROOT, env=env, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    built = target / ('debug' if args.profile == 'dev' else args.profile) / (
        'libswoole.dylib' if sys.platform == 'darwin' else 'libswoole.so')
    extension = out / 'swoole.so'
    shutil.copy2(built, extension)
    php = Path(subprocess.check_output([args.php_config, '--php-binary'], text=True).strip())
    # The PHP extensions the profile's hooks link against, unless PHP has them built in.
    needs = {'sockets': 'sockets', 'curl': 'curl', 'mysqlnd': 'mysqlnd',
             **dict.fromkeys(['pgsql', 'odbc', 'firebird', 'sqlite', 'oci'], 'pdo')}
    builtin = subprocess.check_output([str(php), '-n', '-m'], text=True).lower().split()
    load = sorted({needs[f] for f in features if f in needs} - set(builtin))
    version = subprocess.run([str(php), '-n', *[a for name in load for a in ('-d', f'extension={name}')],
                              '-d', f'extension={extension}', '-r', 'echo SWOOLE_VERSION;'],
                             capture_output=True, text=True)
    if version.stdout != VERSION.removeprefix('v'):
        raise SystemExit(f'{extension} does not load as Swoole {VERSION}:\n{version.stdout}{version.stderr}')
    source_ref = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    (out / 'build-manifest.json').write_text(json.dumps({
        'extension_sha256': hashlib.sha256(extension.read_bytes()).hexdigest(), 'upstream_commit': COMMIT,
        'features': sorted(features), 'profile': args.profile, 'source_ref': source_ref}, indent=1) + '\n')
    print(f'Built cargo: {extension}')


if __name__ == '__main__':
    main()
