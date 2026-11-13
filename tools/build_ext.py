#!/usr/bin/env python3
"""Build swoole.so with Cargo alone (crates/ext): no phpize, configure or make.

crates/ext's build.rs compiles native/ (native/swoole-src holds the C swoole-src bundles), so no
swoole-src checkout is needed; then the extension is checked to load and report the pinned version.
Prints `Built cargo: <path>` (target/ext/swoole.so) and writes the build's config.h beside it, which
the ABI export gate preprocesses with, and target/ext/build-manifest.json, which
tools/ext_identity.py checks gates' engines against. `--php-sockets`, `--stdext`, `--fix-halfclose`
and build_upstream.py's `--configure` switches turn on the crates/ext feature of the same build profile.

`--native` also lays out target/ext/sdk for the native gates, which link fixtures outside PHP:
lib/ holds the Zend-free libswoole_rs_ffi.a and crates/ext's compiled C and C++ archives,
include/ the ABI headers and the build's config.h (see native_flags).
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_upstream import COMMIT, VERSION
from tools.check_migration import MODULES
from tools.harness import ROOT

# crates/ext's features that are swoole-rs-ffi's too: the Zend-free archive's build profile.
FFI_FEATURES = {'sockets', 'stdext', 'halfclose', 'iouring', 'cares', 'pgsql', 'odbc', 'firebird', 'sqlite', 'oci', 'curl', 'ssh2'}

# The ffi features that replace upstream's optional hooks, by the configure switch
# that builds each: the Rust code links what that switch makes the extension link.
HOOK_SWITCHES = {'iouring': '--enable-iouring', 'pgsql': '--enable-swoole-pgsql', 'odbc': '--with-swoole-odbc',
                  'firebird': '--with-swoole-firebird', 'sqlite': '--enable-swoole-sqlite',
                  'oci': '--with-swoole-oracle', 'curl': '--enable-swoole-curl',
                  'ssh2': '--with-swoole-ssh2', 'ftp': '--enable-swoole-ftp'}


def headers_for(modules):
    headers = [ROOT / 'native/include/swoole_rs.h', ROOT / 'native/include/swoole_rs_server_state.h',
               ROOT / 'native/include/swoole_rs_server_strings.h']
    headers.extend((ROOT / 'native/include').glob('swoole_rs_corebase_*.h'))
    for module in modules:
        if module != 'corebase':
            headers.extend((ROOT / 'native/include').glob(f'swoole_rs_{module}_*.h'))
        contract = ROOT / 'native/include' / f'swoole_rs_{module}.h'
        if contract.is_file():
            headers.append(contract)
        headers.extend((ROOT / 'native' / module).rglob('*.h'))
    if len({path.name.casefold() for path in headers}) != len(headers):
        raise ValueError('ABI header basenames overlap in the build prefix')
    return sorted(headers)


def native_sources():
    """Owned C and C++ (native/<module>/*.c and *.cc: what stable Rust can't express), by the path
    it compiles at in a pin checkout, where CMake's src/ glob finds it."""
    paths = sorted((ROOT / 'native').glob('*/*.c')) + sorted((ROOT / 'native').glob('*/*.cc'))
    return {f'src/swoole_rs_{path.parent.name}_{path.name}': path for path in paths}


def zend_free_native_sources():
    """native_sources() without the extension's own glue, which includes PHP's headers: it belongs
    in swoole.so, not in a Zend-free libswoole such as core-tests and the SDK consumers link."""
    return {name: path for name, path in native_sources().items()
            if not any(header in path.read_bytes() for header in (b'#include "swoole_rs_phphost.h"', b'#include "php_swoole.h"'))}


def cargo(arguments, env):
    """Run a Cargo command; return its JSON messages (diagnostics render to stderr)."""
    result = subprocess.run(['cargo', *arguments, '--locked', '--message-format=json-render-diagnostics'],
                            cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True)
    messages = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    result.check_returncode()
    return messages


def sdk(out, features, profile, env):
    """target/ext/sdk: the Zend-free FFI archive (no Zend references, so fixtures link it into
    executables), crates/ext's native archives, and the ABI headers with the build's config.h,
    from crates/ext's build script `out` directory."""
    # `--crate-type staticlib` alone: rustc skips thin LTO whenever an rlib is among the outputs.
    built = cargo(['rustc', '--crate-type', 'staticlib', '--profile', profile, '--no-default-features', '-p', 'swoole-rs-ffi',
                   *(['--features', ','.join(features)] if features else [])], env)
    archive, = [name for m in built if m['reason'] == 'compiler-artifact' and m['target']['name'] == 'swoole_rs_ffi'
                for name in m['filenames'] if name.endswith('.a')]
    root = ROOT / 'target/ext/sdk'
    shutil.rmtree(root, ignore_errors=True)
    (root / 'lib').mkdir(parents=True)
    (root / 'include').mkdir()
    for path in [Path(archive), out / 'libswoole_native_c.a', out / 'libswoole_native_cxx.a']:
        shutil.copy2(path, root / 'lib' / path.name)
    for header in headers_for(MODULES):
        shutil.copy2(header, root / 'include' / header.name)
    shutil.copy2(out / 'config.h', root / 'include/config.h')


def native_flags(archive):
    """Compiler flags for code built against the SDK an archive (`target/ext/sdk/lib/libswoole_rs_ffi.a`)
    belongs to: the SWOOLE_RS_* switches its config.h turns on and its include/, which holds
    native/'s headers and that config.h. A gate's selected side compiles against these, never
    against the pinned headers; its upstream side uses the pin."""
    include = Path(archive).resolve().parents[1] / 'include'
    if not (include / 'config.h').is_file():
        raise ValueError(f'Not an SDK archive (tools/build_ext.py --native): {archive}')
    switches = re.findall(r'^#define (SWOOLE_RS_\w+) 1$', (include / 'config.h').read_text(), re.M)
    return [*('-D' + name for name in switches), '-I' + str(include)]


def native_link(archive):
    """Linker inputs for a fixture that calls the Rust `archive` outside PHP: the archive grouped
    with crates/ext's native archives, which the Rust ops reach, and the libraries swoole.so
    links. Members are pulled in only when referenced, so a fixture never links the Zend glue."""
    lib = Path(archive).resolve().parent
    archives = [str(lib / 'libswoole_native_cxx.a'), str(lib / 'libswoole_native_c.a'), str(lib / Path(archive).name)]
    libraries = shlex.split(subprocess.check_output(
        ['pkg-config', '--libs', 'openssl', 'libbrotlienc', 'libbrotlidec', 'libzstd', 'zlib'], text=True))
    if sys.platform == 'darwin':
        return [*archives, '-Wl,-dead_strip', *libraries]
    return ['-Wl,--start-group', *archives, '-Wl,--end-group', '-Wl,--gc-sections', *libraries, '-pthread', '-ldl', '-lm']


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--profile', default='release', help='Cargo profile (default: release)')
    parser.add_argument('--php-sockets', action='store_true', help='Build with PHP sockets integration')
    parser.add_argument('--stdext', action='store_true', help='Build with the experimental stdext')
    parser.add_argument('--fix-halfclose', action='store_true', help='Build the opt-in half-close repairs')
    parser.add_argument('--configure', action='append', default=[], metavar='ARG',
                        help="A service hook's configure switch, as build_upstream.py takes it; repeatable")
    parser.add_argument('--native', action='store_true', help='Also lay out target/ext/sdk for the native gates')
    args = parser.parse_args()
    out = ROOT / 'target/ext'
    out.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, 'PHP_CONFIG': args.php_config}
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
    messages = cargo(['build', '--profile', args.profile, '-p', 'swoole-rs-ext', '--features', ','.join(features)], env)
    built_by = Path(next(m['out_dir'] for m in messages if m['reason'] == 'build-script-executed'
                         and m['package_id'].split('#')[0].endswith('/ext')))
    shutil.copy2(built_by / 'config.h', out / 'config.h')
    if args.native:
        sdk(built_by, [f for f in features if f in FFI_FEATURES], args.profile, env)
    built, = [name for m in messages if m['reason'] == 'compiler-artifact' and m['target']['name'] == 'swoole'
              for name in m['filenames'] if name.endswith(('.so', '.dylib'))]
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
