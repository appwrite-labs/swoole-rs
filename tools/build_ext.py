#!/usr/bin/env python3
"""Build swoole.so with Cargo alone (crates/ext): no phpize, configure or make.

crates/ext's build.rs compiles native/ and the bundled thirdparty C from the unpatched pin it
checks out in target/ext/src (native/swoole-src owns the files it changes), then the extension
is checked to load and report the pinned version.
Prints `Built cargo: <path>` (target/ext/swoole.so) and writes target/ext/build-manifest.json,
which tools/ext_identity.py checks gates' engines against. `--php-sockets`, `--stdext`, `--fix-halfclose`
and build_hybrid's `--configure` switches turn on the crates/ext feature of the same build profile.

`--native` also lays out target/ext/sdk for the native gates, which link fixtures outside PHP:
lib/ holds the Zend-free libswoole_rs_ffi.a and crates/ext's compiled C and C++ archives,
include/ the ABI headers, and target/ext/src/config.h is the build's config.h. The gates still
compile against the patched tree, which it applies in target/ext/patched (see native_tree) until
they move off it (#2200).
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
from tools.build_hybrid import HOOK_SWITCHES, MODULES, headers_for, module_patches, zend_free_native_sources
from tools.build_upstream import COMMIT, VERSION
from tools.harness import ROOT
from tools.patches import apply, clone_pin, git

# crates/ext's features that are swoole-rs-ffi's too: the Zend-free archive's build profile.
FFI_FEATURES = {'sockets', 'stdext', 'halfclose', 'iouring', 'cares', 'pgsql', 'odbc', 'firebird', 'sqlite', 'oci', 'curl', 'ssh2'}


def cargo(arguments, env):
    """Run a Cargo command; return its JSON messages (diagnostics render to stderr)."""
    result = subprocess.run(['cargo', *arguments, '--locked', '--message-format=json-render-diagnostics'],
                            cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True)
    messages = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
    result.check_returncode()
    return messages


def sdk(tree, messages, features, profile, env):
    """target/ext/sdk: the Zend-free FFI archive (no Zend references, so fixtures link it into
    executables), crates/ext's native archives, and the ABI headers; and config.h in the tree."""
    out = Path(next(m['out_dir'] for m in messages if m['reason'] == 'build-script-executed'
                    and m['package_id'].split('#')[0].endswith('/ext')))
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
    apply(tree.parent / 'patched')
    for directory in (tree, tree.parent / 'patched'):
        shutil.copy2(out / 'config.h', directory / 'config.h')


def checkout(tree):
    """The pin, unpatched, in `tree`: kept when it is already there and clean."""
    if (tree / '.git').exists() and git(tree, 'status', '--porcelain', '--untracked-files=no') == '' \
            and git(tree, 'rev-parse', 'HEAD').strip() == COMMIT:
        return
    shutil.rmtree(tree, ignore_errors=True)
    clone_pin(tree)
    git(tree, 'checkout', '-q', COMMIT)


def native_tree(archive):
    """The tree an SDK archive (`target/ext/sdk/lib/libswoole_rs_ffi.a`) belongs to: the patched
    source tree and the compiler flags for its headers, i.e. the patched include/, the ABI
    headers and the SWOOLE_RS_* switches its config.h turns on. A gate's selected side compiles
    against these, never against the pinned headers; its upstream side uses the pin."""
    ext = Path(archive).resolve().parents[2]
    source, abi = ext / 'patched', ext / 'sdk/include'
    if not (source / 'config.h').is_file() or not abi.is_dir():
        raise ValueError(f'Not an SDK archive (tools/build_ext.py --native): {archive}')
    switches = re.findall(r'^#define (SWOOLE_RS_\w+) 1$', (source / 'config.h').read_text(), re.M)
    return source, [*('-D' + name for name in switches), '-I' + str(source / 'include'), '-I' + str(abi)]


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


def overlay_native_tree(archive, tree):
    """Copy every file the patch set patched over `tree`, a pinned checkout (such as one staged
    with upstream's core-tests), delete the ones it deleted, and add the Zend-free native sources.
    Returns the CMake lines that give the tree's existing targets native_tree's switches and
    include directories."""
    source, flags = native_tree(archive)
    for directory in (ROOT / 'migration/patches').iterdir():
        for patch in module_patches(directory):
            for name in patch['upstream_paths']:
                target = Path(tree) / name
                if not (source / name).exists():
                    target.unlink(missing_ok=True)
                elif not target.is_file() or (source / name).read_bytes() != target.read_bytes():
                    # A tree may lack the file: an archive of the pin export-ignores core-tests/.
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(source / name, target)
    for name, path in zend_free_native_sources().items():
        (Path(tree) / name).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, Path(tree) / name)
    # core-tests include a header the patch set folded into another one; they include the survivor.
    for path in (Path(tree) / 'core-tests').rglob('*'):
        text = path.read_text() if path.suffix in ('.h', '.cpp') else ''
        folded = text
        for header, survivor in (('swoole_process_pool.h', 'swoole_server.h'), ('swoole_config.h', 'swoole.h'),
                                 ('swoole_coroutine_channel.h', 'swoole_coroutine.h'),
                                 ('swoole_socket_impl.h', 'swoole_rs_coroutine_socket.h'),
                                 ('swoole_coroutine_socket.h', 'swoole_rs_coroutine_socket.h')):
            folded = folded.replace(f'#include "{header}"', f'#include "{survivor}"')
        if folded != text:
            path.write_text(folded)
    # add_definitions and include_directories reach targets defined before them; add_compile_options does not.
    defines = ' '.join(flag for flag in flags if flag.startswith('-D'))
    includes = ' '.join(f'"{flag[2:]}"' for flag in flags if flag.startswith('-I'))
    return f'add_definitions({defines})\ninclude_directories({includes})\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--php-config', default=os.environ.get('PHP_CONFIG', 'php-config'))
    parser.add_argument('--profile', default='release', help='Cargo profile (default: release)')
    parser.add_argument('--php-sockets', action='store_true', help='Build with PHP sockets integration')
    parser.add_argument('--stdext', action='store_true', help='Build with the experimental stdext')
    parser.add_argument('--fix-halfclose', action='store_true', help='Build the opt-in half-close repairs')
    parser.add_argument('--configure', action='append', default=[], metavar='ARG',
                        help="A service hook's configure switch, as build_hybrid.py takes it; repeatable")
    parser.add_argument('--native', action='store_true', help='Also lay out target/ext/sdk for the native gates')
    args = parser.parse_args()
    out = ROOT / 'target/ext'
    tree = out / 'src'
    checkout(tree)
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
    messages = cargo(['build', '--profile', args.profile, '-p', 'swoole-rs-ext', '--features', ','.join(features)], env)
    if args.native:
        sdk(tree, messages, [f for f in features if f in FFI_FEATURES], args.profile, env)
    # CARGO_BUILD_TARGET (coverage.yml) puts the library under target/<triple>/.
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'), os.environ.get('CARGO_BUILD_TARGET', ''))
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
