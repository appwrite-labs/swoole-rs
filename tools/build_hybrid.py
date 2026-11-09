#!/usr/bin/env python3
"""Build a pinned Swoole extension with an optional, version-checked Rust C ABI."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.build_upstream import COMMIT, VERSION, add_build_arguments, build_extension, extension_configuration, require_build_space, compiler_cache_identity
from tools.check_migration import validate
from tools.harness import ROOT, write_report
from tools.harness import exclusive
from tools.private_copy import copy_private

MODULES = {'websocket': ('M02', 1), 'buffer': ('M01', 2), 'timer': ('M03', 4),
           'base64': ('M09', 8), 'crc32': ('M10', 16), 'error': ('M11', 32), 'protocols': ('M13', 128), 'memory': ('M12', 64),
           'http': ('M04', 256), 'utilities': ('M14', 512), 'locks': ('M16', 1024), 'wrappers': ('M17', 2048), 'framing': ('M18', 4096), 'corebase': ('M19', 8192), 'reactor': ('M05', 16384), 'coroutine': ('M06', 32768), 'hooks': ('M07', 65536), 'ossupport': ('M20', 131072), 'tls': ('M21', 262144), 'process': ('M08', 524288), 'clients': ('M22', 1048576), 'httpbindings': ('M23', 2097152), 'http2': ('M24', 4194304), 'ports': ('M25', 8388608), 'phphelpers': ('M26', 16777216), 'phpcollections': ('M27', 33554432), 'phpevents': ('M28', 67108864), 'phpcoroutines': ('M29', 134217728)}
# Every Rust build has every module: none has a configure switch or a cargo feature.
# Graduated modules (docs/migration.md#graduation) have their gates run on every dev.py run.
GRADUATED = ('base64', 'buffer', 'corebase', 'crc32', 'error', 'framing', 'http', 'protocols', 'websocket', 'timer')
# The integration patch that adds the --with(out)-swoole-rs switch; later patches to
# the same files call into Rust.
BRIDGE_PATCH = 'migration/patches/integration/0001-rust-bridge.patch'
# The ffi features that replace upstream's optional hooks, by the configure switch
# that builds each: the Rust code links what that switch makes the extension link.
HOOK_SWITCHES = {'iouring': '--enable-iouring', 'pgsql': '--enable-swoole-pgsql', 'odbc': '--with-swoole-odbc',
                  'firebird': '--with-swoole-firebird', 'sqlite': '--enable-swoole-sqlite',
                  'oci': '--with-swoole-oracle', 'curl': '--enable-swoole-curl',
                  'ssh2': '--with-swoole-ssh2', 'ftp': '--enable-swoole-ftp'}
# The build's two Rust archives in rust-prefix/lib, by the input that records each: the
# Zend-free one the native gates link, and the PHP extension's (with LTO unless --dev, #889).
ARCHIVES = {'libswoole_rs_ffi.a': 'rust_archive_sha256', 'libswoole_rs_php.a': 'rust_php_archive_sha256'}


def profile_suffix(args):
    return ('-php-sockets' if getattr(args, 'php_sockets', False) else '') + ('-stdext' if getattr(args, 'stdext', False) else '')


def build_alias(backend, suffix='', *, dev=False):
    """target/hybrid/<alias>: rust or upstream, with the profile's suffix."""
    return ('dev-' if dev else '') + backend + suffix


def headers_for(modules):
    headers = [ROOT / 'native/include/swoole_rs.h', ROOT / 'native/include/swoole_rs_server_state.h',
               ROOT / 'native/include/swoole_rs_server_view.h', ROOT / 'native/include/swoole_rs_server_records.h', ROOT / 'native/include/swoole_rs_port_state.h', ROOT / 'native/include/swoole_rs_server_shared.h', ROOT / 'native/include/swoole_rs_server_strings.h']
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


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def native_compilation(source, log):
    """Bind configured flags and observed compiler commands, without executing Make."""
    names = {'CC', 'CXX', 'CFLAGS', 'CFLAGS_CLEAN', 'CXXFLAGS', 'CXXFLAGS_CLEAN',
             'CPPFLAGS', 'LDFLAGS', 'EXTRA_CFLAGS', 'EXTRA_CXXFLAGS', 'EXTRA_LDFLAGS'}
    variables = {}
    for line in (source / 'Makefile').read_text().splitlines():
        if line.startswith('\t'):
            continue
        key, separator, value = line.partition('=')
        if separator and key.strip() in names:
            variables[key.strip()] = value.strip()
    commands = [line for line in log.read_text().splitlines() if '--mode=compile ' in line]
    return {'scope': 'Configured flags and commands observed in this successful invocation; no automatic profile-equivalence or performance claim',
            'files_sha256': {'Makefile': digest(source / 'Makefile'), 'config.h': digest(source / 'config.h'),
                             'build.log': digest(log)},
            'make_variables': variables, 'compile_command_count': len(commands),
            'compile_commands_sha256': hashlib.sha256('\n'.join(commands).encode()).hexdigest()}


def cached_build_matches(record, inputs, export_source_ref=None):
    """An export requires a producer from this cohort, even for identical inputs."""
    return (record['inputs'] == inputs and
            (export_source_ref is None or record.get('producer_source_ref') == export_source_ref))


def export_build(manifest, source, destination, source_ref):
    """Atomically copy a completed immutable build; never overwrite evidence."""
    manifest, source, destination = Path(manifest), Path(source).resolve(strict=True), Path(destination)
    manifest_bytes = manifest.read_bytes()
    record = json.loads(manifest_bytes)
    if record['build_mode'] != 'immutable':
        raise ValueError('Only immutable builds may be exported')
    producer = record.get('producer_source_ref')
    if not producer:
        raise ValueError('Build producer source is unknown in this legacy manifest; preserve existing exports or rebuild before new publication')
    if producer != source_ref:
        raise ValueError('Build producer source differs; cross-cohort publication requires input-closure proof and is not supported')
    files = {'swoole.so': source / 'modules/swoole.so', 'config.h': source / 'config.h',
             'Makefile': source / 'Makefile', 'build.log': source.parent / 'build.log',
             'build-manifest.json': manifest}
    archives = {name: record['inputs'].get(key) for name, key in ARCHIVES.items()}
    files.update({name: source.parent / 'rust-prefix/lib' / name for name, sha256 in archives.items() if sha256})
    before = {name: digest(path) for name, path in files.items()}
    if (before['build-manifest.json'] != hashlib.sha256(manifest_bytes).hexdigest()
            or before['swoole.so'] != record['extension_sha256']
            or any(sha256 and before[name] != sha256 for name, sha256 in archives.items())):
        raise ValueError('Completed build artifacts differ from their manifest')
    destination.parent.mkdir(parents=True, exist_ok=True)
    with exclusive(destination.parent / '.export.lock'):
        if destination.exists():
            raise FileExistsError('Export already exists: ' + str(destination))
        with tempfile.TemporaryDirectory(prefix='.export-', dir=destination.parent) as temporary:
            snapshot = Path(temporary) / 'snapshot'
            snapshot.mkdir()
            for name, path in files.items():
                copy_private(path, snapshot / name)
            if before != {name: digest(path) for name, path in files.items()} or before != {name: digest(snapshot / name) for name in files}:
                raise ValueError('Build artifacts changed during export')
            receipt = {'source_ref': producer, 'producer_source_ref': producer,
                       'observation_scope': 'Verified copies of a completed build; source correspondence and runtime qualification remain separate gates',
                       'files': {name: {'source': str(path), 'sha256': before[name], 'bytes': (snapshot / name).stat().st_size}
                                 for name, path in files.items()},
                       'native_compilation_observation': native_compilation(snapshot, snapshot / 'build.log')}
            write_report(snapshot / 'export.json', receipt)
            snapshot.rename(destination)
    return receipt


def rust_sources(extra=()):
    """Hash selected first-party Cargo manifests/source trees, including path dependencies."""
    command = ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
               '--manifest-path', str(ROOT / 'crates/ffi/Cargo.toml'), '--no-default-features',
               '--features', ','.join(['php', *extra])]
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT, text=True))
    packages = {package['id']: package for package in metadata['packages']}
    nodes = {node['id']: node for node in metadata['resolve']['nodes']}
    pending, selected = [metadata['resolve']['root']], set()
    files = {ROOT / 'Cargo.toml', ROOT / 'Cargo.lock'}
    files.update(path for name in ['config', 'config.toml'] if (path := ROOT / '.cargo' / name).is_file())
    while pending:
        package_id = pending.pop()
        if package_id in selected:
            continue
        selected.add(package_id)
        package = packages[package_id]
        pending += [dependency['pkg'] for dependency in nodes[package_id]['deps']
                    if any(kind['kind'] in {None, 'build'} for kind in dependency['dep_kinds'])]
        if package['source'] is not None:
            continue
        manifest = Path(package['manifest_path']).resolve()
        if ROOT not in manifest.parents:
            raise ValueError('First-party Rust dependency is outside the source snapshot: ' + str(manifest))
        files.add(manifest)
        files.update(path for path in (manifest.parent / 'src').rglob('*') if path.is_file())
        files.update(Path(target['src_path']).resolve() for target in package['targets'] if 'custom-build' in target['kind'])
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(files)}


def stdext_enabled(args):
    """Match configure's final stdext selection, including explicit overrides."""
    enabled = getattr(args, 'stdext', False)
    for option in getattr(args, 'configure', []):
        if option in {'--disable-swoole-stdext', '-disable-swoole-stdext'}:
            enabled = False
        elif option in {'--enable-swoole-stdext', '-enable-swoole-stdext'}:
            enabled = True
        elif option.startswith(('--enable-swoole-stdext=', '-enable-swoole-stdext=')):
            enabled = option.partition('=')[2] != 'no'
    return enabled


def rust_archive(php_config='php-config', extra=(), *, php=True, profile='release'):
    """Use Cargo's actual static-library output, including configured target directories.
    zend-sys binds the headers of the PHP the extension is built for (PHP_CONFIG). Without
    `php` the archive holds no Zend references: the native gates link it into executables.
    `--crate-type staticlib` alone: crates/ext needs the crate as an rlib too, and rustc skips
    thin LTO whenever an rlib is among the outputs, which costs ~330 instructions a request (#2005)."""
    command = ['cargo', 'rustc', '--crate-type', 'staticlib', '--locked', *(['--release'] if profile == 'release' else ['--profile', profile]), '--no-default-features', '-p', 'swoole-rs-ffi',
               '--message-format=json-render-diagnostics']
    if features := (['php'] if php else []) + list(extra):
        command += ['--features', ','.join(features)]
    result = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, text=True,
                            env={**os.environ, 'PHP_CONFIG': php_config})
    artifacts = []
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message['reason'] == 'compiler-message':
            print(message['message'].get('rendered', ''), end='', file=sys.stderr)
        if message['reason'] == 'compiler-artifact' and message['target']['name'] == 'swoole_rs_ffi':
            artifacts += [(Path(name).resolve(), message) for name in message['filenames'] if name.endswith('.a')]
    result.check_returncode()
    if len(artifacts) != 1:
        raise ValueError('Cargo did not identify exactly one FFI static archive')
    archive, artifact = artifacts[0]
    return archive, {'command': command, 'artifact': artifact}


def output(command, cwd=ROOT):
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def module_patches(directory):
    """A patch directory's patches in application order, which is filename order: older ones are
    numbered (0001-topic.patch), newer ones timestamped (20261102T153012-topic.patch), so adding
    one never renames another. Each row is its file name, sha256 and the upstream files its diff
    headers name."""
    rows = []
    for path in sorted(Path(directory).glob('*.patch'), key=lambda path: path.name):
        body = path.read_bytes()
        actual = []
        line_delta = 0
        for line in body.decode().splitlines():
            if line.startswith('diff --git '):
                line_delta = 0
                fields = shlex.split(line)
                if len(fields) != 4 or not fields[2].startswith('a/') or fields[2][2:] != fields[3][2:]:
                    raise ValueError('Renames or malformed patch headers require review')
                target = fields[2][2:]
                if Path(target).is_absolute() or '..' in Path(target).parts:
                    raise ValueError(f'Unsafe patch target: {target}')
                actual.append(target)
            elif line.startswith('@@ '):
                hunk = re.match(r'@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@', line)
                if not hunk:
                    raise ValueError(f'Malformed overlay hunk: {path}')
                old, removed, new, added = [int(value) if value is not None else 1 for value in hunk.groups()]
                if new - bool(added) != old - bool(removed) + line_delta:
                    raise ValueError(f'Overlay new-line offset differs: {path}: {line}')
                line_delta += added - removed
        if not actual:
            raise ValueError(f'Patch names no upstream file: {path}')
        rows.append({'path': path.name, 'sha256': hashlib.sha256(body).hexdigest(), 'upstream_paths': actual})
    return rows


def patches_for(modules):
    """Each module's patches, in application order, with paths relative to the checkout."""
    collected = []
    for module in modules:
        directories = [path.rstrip('/') for path in module['owned_paths'] if path.startswith('migration/patches/')]
        if len(directories) != 1:
            raise ValueError(f'A module must own exactly one patch directory: {module["id"]}')
        collected += [{**row, 'path': f'{directories[0]}/{row["path"]}'} for row in module_patches(ROOT / directories[0])]
    return collected


def hybrid_tree(archive):
    """The hybrid build an archive came from (`<build>/rust-prefix/lib/libswoole_rs_ffi.a`): its
    fully patched source tree and the compiler flags for its headers, i.e. the patched include/,
    the Rust ABI headers and the module switches its config.h turns on. A gate's hybrid side
    compiles against these, never against the pinned headers; its upstream side uses the pin."""
    build = Path(archive).resolve().parents[2]
    source, abi = build / 'src', build / 'rust-prefix/include'
    if not (source / 'config.h').is_file() or not abi.is_dir():
        raise ValueError(f'Not a hybrid build archive (<build>/rust-prefix/lib/): {archive}')
    switches = re.findall(r'^#define (SWOOLE_RS_\w+) 1$', (source / 'config.h').read_text(), re.M)
    return source, [*('-D' + name for name in switches), '-I' + str(source / 'include'), '-I' + str(abi)]


def hybrid_link(archive, directory):
    """Linker inputs for a fixture that calls the Rust `archive` outside PHP: the archive grouped
    with an archive of the extension's own native objects, which the Rust ops reach (the shims and
    the upstream C++ still in the build), and the libraries the extension links. Members are pulled
    in only when referenced, so a fixture never links the extension's Zend glue it doesn't reach."""
    source = Path(archive).resolve().parents[2] / 'src'
    make = {}
    for line in (source / 'Makefile').read_text().splitlines():
        key, separator, value = line.partition(' = ')
        if separator and not line.startswith('\t'):
            make[key] = value
    objects = [str(source / Path(lo).parent / '.libs' / (Path(lo).stem + '.o'))
               for lo in make['shared_objects_swoole'].split()]
    native = Path(directory) / 'libswoole_native.a'
    native.unlink(missing_ok=True)
    subprocess.run(['ar', 'rcs', str(native), *objects], check=True)
    libraries = [flag for flag in shlex.split(make.get('SWOOLE_SHARED_LIBADD', '')) if flag.startswith(('-l', '-L'))]
    if sys.platform == 'darwin':
        return [str(native), str(Path(archive).resolve()), '-Wl,-dead_strip', *libraries]
    return ['-Wl,--start-group', str(native), str(Path(archive).resolve()), '-Wl,--end-group',
            '-Wl,--gc-sections', *libraries, '-pthread', '-ldl', '-lm']


def native_sources():
    """Owned C and C++ (native/<module>/*.c and *.cc: what stable Rust can't express), by the path
    it compiles at in the patched tree, where config.m4's and CMake's src/ globs find it."""
    paths = sorted((ROOT / 'native').glob('*/*.c')) + sorted((ROOT / 'native').glob('*/*.cc'))
    return {f'src/swoole_rs_{path.parent.name}_{path.name}': path for path in paths}


def zend_free_native_sources():
    """native_sources() without the extension's own glue, which includes PHP's headers: it belongs
    in swoole.so, not in a Zend-free libswoole such as core-tests and the SDK consumers link."""
    return {name: path for name, path in native_sources().items()
            if not any(header in path.read_bytes() for header in (b'#include "swoole_rs_phphost.h"', b'#include "php_swoole.h"'))}


def stage_native_sources(source):
    sources = native_sources()
    # An incremental tree still holds the copy of a native file deleted since it was staged.
    for stale in [*(source / 'src').glob('swoole_rs_*.cc'), *(source / 'src').glob('swoole_rs_*.c')]:
        if f'src/{stale.name}' not in sources:
            stale.unlink()
    for name, path in sources.items():
        if not (source / name).exists() or (source / name).read_bytes() != path.read_bytes():
            (source / name).parent.mkdir(exist_ok=True)  # the patched tree may have no src/ left
            shutil.copyfile(path, source / name)


def overlay_hybrid_tree(archive, tree):
    """Copy every file the archive's hybrid build patched over `tree`, a pinned checkout (such as
    one staged with upstream's core-tests), and delete the ones it deleted. Returns the CMake
    lines that give the tree's existing targets hybrid_tree's switches and include directories."""
    source, flags = hybrid_tree(archive)
    for directory in (ROOT / 'migration/patches').iterdir():
        for patch in module_patches(directory):
            for name in patch['upstream_paths']:
                target = Path(tree) / name
                if not (source / name).exists():
                    target.unlink(missing_ok=True)
                elif not target.is_file() or (source / name).read_bytes() != target.read_bytes():
                    # A tree may lack the file: `git archive` export-ignores core-tests/.
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(source / name, target)
    for name in zend_free_native_sources():
        shutil.copyfile(source / name, Path(tree) / name)
    # core-tests include a header the hybrid folded into another one; they include the survivor.
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


def apply_patch_verified(source, patch, targets, expected_sha256=None):
    """Apply inside a private tree and prove every declared destination changed."""
    source, patch = Path(source).resolve(), Path(patch).resolve()
    if any(Path(name).is_absolute() or '..' in Path(name).parts for name in targets):
        raise ValueError('Patch requires safe, explicit file destinations')
    paths = {name: source / name for name in targets}
    if not paths or any(source not in path.resolve().parents for path in paths.values()):
        raise ValueError('Patch requires safe, explicit file destinations')
    patch_sha = digest(patch)
    if expected_sha256 is not None and patch_sha != expected_sha256:
        raise ValueError('Patch changed before application')
    before = {name: digest(path) if path.is_file() else None for name, path in paths.items()}
    # A staging directory under an ignored parent worktree otherwise lets Git
    # silently skip every hunk with exit0. Give it its own repository boundary.
    if not (source / '.git').exists():
        subprocess.run(['git', 'init', '-q', str(source)], check=True)
    commands = []
    for options in [['--check'], []]:
        command = ['git', 'apply', *options, str(patch)]
        result = subprocess.run(command, cwd=source, capture_output=True, text=True)
        commands.append({'command': command, 'returncode': result.returncode,
                         'stdout': result.stdout, 'stderr': result.stderr})
        if result.returncode:
            raise RuntimeError(f'Patch application failed: {commands}')
    if digest(patch) != patch_sha:
        raise ValueError('Patch changed during application')
    after = {name: digest(path) if path.is_file() else None for name, path in paths.items()}
    if any(before[name] == after[name] for name in paths):
        raise ValueError(f'Patch left a declared destination unchanged: {before} -> {after}')
    return {'patch_sha256': patch_sha, 'before': before, 'after': after, 'commands': commands}


def switch_link(path, destination):
    if path.exists() and not path.is_symlink():
        raise ValueError(f'Refusing to replace a non-symlink build artifact: {path}')
    temporary = path.with_name(path.name + '.next')
    if temporary.exists() or temporary.is_symlink():
        raise ValueError(f'Unexpected temporary link: {temporary}')
    temporary.symlink_to(destination, target_is_directory=True)
    temporary.replace(path)


def refresh_dev_sources(reference, source, previous, patches):
    """Stage overlays from Git objects; preserve mtimes of unchanged native sources."""
    targets = sorted({path for patch in [*previous, *patches] for path in patch['upstream_paths']})
    with tempfile.TemporaryDirectory(prefix='swr-dev-overlay-') as directory:
        staged = Path(directory)
        for target in targets:
            path = staged / target
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(subprocess.check_output(['git', 'show', f'{COMMIT}:{target}'], cwd=reference))
        for patch in patches:
            apply_patch_verified(staged, ROOT / patch['path'], patch['upstream_paths'], patch.get('sha256'))
        for target in targets:
            path = source / target
            if not (staged / target).exists():
                path.unlink(missing_ok=True)
            elif not path.exists() or path.read_bytes() != (staged / target).read_bytes():
                shutil.copyfile(staged / target, path)


def configuration_key(inputs, development=False):
    configuration = inputs
    if development:
        configuration = {key: value for key, value in inputs.items()
                         if key not in {'patches', 'rust_sources_sha256', 'native_sources_sha256', *ARCHIVES.values()}}
        # Module-local declarations affect consumer compilation, not configure.
        # Prefix staging below updates their mtimes; native dependency files rebuild callers.
        configuration['headers_sha256'] = {path: value for path, value in inputs.get('headers_sha256', {}).items()
                                            if not path.startswith('native/') or path.startswith('native/include/')}
        # config.m4 globs src/ at configure, so adding or deleting a native file reconfigures,
        # and so does deleting a header, which the old tree's dependency files still name.
        configuration['native_sources'] = sorted(inputs.get('native_sources_sha256', {}))
        configuration['headers'] = sorted(inputs.get('headers_sha256', {}))
        configuration['configure_patches'] = [patch for patch in inputs['patches'] if
                                              any(path in {'config.m4', 'CMakeLists.txt'} for path in patch['upstream_paths'])]
    return hashlib.sha256(json.dumps(configuration, sort_keys=True).encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_build_arguments(parser)
    parser.add_argument('--backend', choices=['rust', 'upstream'], default='rust', help='Rust ABI seam enabled, or explicit upstream fallback')
    parser.add_argument('--fix-halfclose', action='store_true', help='Build the opt-in half-close repairs (M15: the halfclose cargo feature)')
    parser.add_argument('--dev', action='store_true', help='Reuse a private incremental tree; never publishes integration artifacts')
    parser.add_argument('--export', type=Path, help='Publish verified immutable copies to a new directory; refuses replacement')
    parser.add_argument('--native-build-root', type=Path, help='Private native build directory, including an explicitly mounted RAM filesystem')
    args = parser.parse_args()
    if args.dev and args.export:
        parser.error('--export requires an immutable build, without --dev')
    if args.export and args.export.exists():
        parser.error('Export already exists: ' + str(args.export))
    with exclusive(ROOT / 'target/hybrid/.builder.lock'):
        return build(args, parser)


def build(args, parser):
    require_build_space(ROOT)
    started = time.monotonic()
    timings = {}
    args.module = sorted(MODULES) if args.backend == 'rust' else []
    if args.jobs < 1:
        parser.error('--jobs must be positive')
    reference = ROOT / 'target/upstream/src'
    if not reference.is_dir():
        parser.error('Run python3 tools/build_upstream.py first to fetch/build the pinned reference')
    if output(['git', 'rev-parse', 'HEAD'], reference) != COMMIT:
        raise ValueError('Reference pin differs; refusing to build')
    if output(['git', 'status', '--porcelain', '--untracked-files=no'], reference):
        raise ValueError('Reference has tracked modifications; refusing to build')
    plan = json.loads((ROOT / 'migration/modules.json').read_text())
    validate(plan)
    selected = {'M00', *(MODULES[name][0] for name in args.module)}
    patches = patches_for([module for module in plan['modules'] if module['id'] in selected])
    if args.backend == 'upstream':
        # The upstream fallback links no Rust: it takes only the bridge switch, so the
        # sources that call into Rust stay pristine (#801).
        patches = [patch for patch in patches if patch['path'] == BRIDGE_PATCH]
    archives, rust_inputs, rust_compilation, rust_php_compilation = {}, None, None, None
    # The Rust io_uring engine replaces upstream's when the build enables io_uring.
    cargo_extra = [feature for feature, switch in HOOK_SWITCHES.items()
                   if any(flag.startswith(switch) for flag in args.configure)]
    cares = next((flag for flag in reversed(args.configure)
                  if flag in {'--enable-cares', '--disable-cares'} or flag.startswith('--enable-cares=')), None)
    if cares not in {None, '--disable-cares', '--enable-cares=no'}:
        cargo_extra.append('cares')
    if args.backend == 'rust':
        if getattr(args, 'php_sockets', False):
            cargo_extra.append('sockets')
        if stdext_enabled(args):
            cargo_extra.append('stdext')
        if getattr(args, 'fix_halfclose', False):
            cargo_extra.append('halfclose')
        rust_inputs = rust_sources(cargo_extra)
        cargo_started = time.monotonic()
        staging = ROOT / 'target/hybrid/.archives'
        staging.mkdir(parents=True, exist_ok=True)
        for name, php, profile in [('libswoole_rs_ffi.a', False, 'release'),
                                   ('libswoole_rs_php.a', True, 'release' if args.dev else 'release-lto')]:
            archive, compilation = rust_archive(args.php_config, cargo_extra, php=php, profile=profile)
            # Both --dev archives are Cargo's one release output: keep each before the next build.
            archives[name] = staging / name
            copy_private(archive, archives[name])
            if php:
                rust_php_compilation = compilation
            else:
                rust_compilation = compilation
        if rust_sources(cargo_extra) != rust_inputs:
            raise ValueError('Rust sources changed during compilation; refusing to attest the archive')
        timings['cargo'] = round(time.monotonic() - cargo_started, 4)
    switches = ['--with-swoole-rs={BUILD_PREFIX}'] if args.backend == 'rust' else ['--without-swoole-rs']
    effective_configure, effective_env = extension_configuration(args, switches)
    headers = headers_for(args.module)
    inputs = {'php_sockets_requested': getattr(args, 'php_sockets', False),
              'compiler_cache': compiler_cache_identity(args), 'fix_halfclose': getattr(args, 'fix_halfclose', False), 'modules': args.module, 'builder_sha256': digest(__file__), 'php_builder_sha256': digest(ROOT / 'tools/build_upstream.py'),
              'effective_configure': effective_configure, 'effective_cppflags': effective_env['CPPFLAGS'],
              'upstream_commit': COMMIT, 'backend': args.backend, 'patches': patches,
              **{key: digest(archives[name]) if args.backend == 'rust' else None for name, key in ARCHIVES.items()},
              'rust_sources_sha256': rust_inputs,
              'headers_sha256': {str(path.relative_to(ROOT)): digest(path) for path in headers},
              'native_sources_sha256': {name: digest(path) for name, path in native_sources().items()} if args.backend == 'rust' else {},
              'php': output([args.php_config, '--version']),
              'php_config': output([args.php_config, '--configure-options']),
              'phpize': str(Path(shutil.which(args.phpize) or args.phpize).resolve()),
              'rustc': output(['rustc', '--version']),
              'compiler': output(shlex.split(os.environ.get('CXX', 'c++')) + ['--version']),
              'openssl_dir': args.openssl_dir,
              'environment': {key: os.environ.get(key) for key in ['CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'CPPFLAGS', 'LDFLAGS']}}
    fingerprint = configuration_key(inputs, args.dev)
    native_root = getattr(args, 'native_build_root', None) or ROOT / ('target/hybrid/dev' if args.dev else 'target/hybrid/builds')
    native_root = native_root.resolve()
    if native_root == reference.resolve() or reference.resolve() in native_root.parents:
        parser.error('Native builds must stay outside the pinned reference')
    native_root.mkdir(parents=True, exist_ok=True)
    require_build_space(native_root)
    build = native_root / fingerprint
    source = build / 'src'
    state_path = build / 'state.json'
    diff_hash = None
    if source.exists():
        if not state_path.is_file():
            raise ValueError(f'Incomplete build preparation at {build}; inspect it before removing/retrying')
        state = json.loads(state_path.read_text())
        diff_hash = hashlib.sha256(output(['git', 'diff', '--binary', 'HEAD'], source).encode()).hexdigest()
        if (not args.dev and state['inputs'] != inputs) or state['source_diff_sha256'] != diff_hash or output(['git', 'rev-parse', 'HEAD'], source) != COMMIT:
            raise ValueError(f'Build source changed at {source}; preserve it and use a fresh build before retrying')
        if args.dev and state['inputs']['patches'] != patches:
            refresh_dev_sources(reference, source, state['inputs']['patches'], patches)
            diff_hash = hashlib.sha256(output(['git', 'diff', '--binary', 'HEAD'], source).encode()).hexdigest()
    else:
        build.mkdir(parents=True, exist_ok=True)
        subprocess.run(['git', 'clone', '--shared', '--no-checkout', str(reference), str(source)], check=True)
        subprocess.run(['git', '-C', str(source), 'checkout', '--detach', COMMIT], check=True)
        for patch in patches:
            proof = apply_patch_verified(source, ROOT / patch['path'], patch['upstream_paths'], patch['sha256'])
            write_report(build / ('patch-' + Path(patch['path']).parent.name + '-' + Path(patch['path']).stem + '.json'), proof)
        diff_hash = hashlib.sha256(output(['git', 'diff', '--binary', 'HEAD'], source).encode()).hexdigest()
    if args.backend == 'rust':
        stage_native_sources(source)
    write_report(state_path, {'inputs': inputs, 'source_diff_sha256': diff_hash})
    prefix = build / 'rust-prefix'
    extra = ['--without-swoole-rs']
    if args.backend == 'rust':
        (prefix / 'include').mkdir(parents=True, exist_ok=True)
        (prefix / 'lib').mkdir(parents=True, exist_ok=True)
        for old in (prefix / 'include').glob('*.h'):
            if old.name not in {path.name for path in headers}:
                old.unlink()
        for include in headers:
            destination = prefix / 'include' / include.name
            if not destination.exists() or digest(include) != digest(destination):
                copy_private(include, destination)
        for name, archive in archives.items():
            destination = prefix / 'lib' / name
            if not destination.exists() or digest(archive) != digest(destination):
                copy_private(archive, destination)
        extra = [switch.replace('{BUILD_PREFIX}', str(prefix)) for switch in switches]
    library = source / 'modules/swoole.so'
    record_path = build / 'build-manifest.json'
    cached = False
    if record_path.exists() and library.exists():
        record = json.loads(record_path.read_text())
        if record['extension_sha256'] != digest(library):
            raise ValueError('Cached extension checksum changed; refusing to publish it')
        cached = cached_build_matches(record, inputs,
                                      output(['git', 'rev-parse', 'HEAD']) if args.export else None)
    if not cached:
        incremental = args.dev and record_path.exists() and (source / 'Makefile').exists()
        producer_source_ref = output(['git', 'rev-parse', 'HEAD'])
        configure = build_extension(source, build / 'build.log', args, extra, incremental=incremental, timings=timings)
        if output(['git', 'rev-parse', 'HEAD']) != producer_source_ref:
            raise ValueError('Source commit changed during native compilation; refusing to attest producer identity')
        record = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(),
                  'build_mode': 'development' if args.dev else 'immutable',
                  'producer_source_ref': producer_source_ref,
                  'rust_compilation': rust_compilation, 'rust_php_compilation': rust_php_compilation,
                  'native_compilation': native_compilation(source, build / 'build.log'),
                  'upstream_version': VERSION, 'inputs': inputs, 'source_diff_sha256': diff_hash,
                  'configure': configure, 'builder_sha256': digest(__file__), 'php_builder_sha256': digest(ROOT / 'tools/build_upstream.py'), 'extension': str(library), 'extension_sha256': digest(library),
                  'log': str(build / 'build.log'), 'migrated_feature_bits': sum(MODULES[name][1] for name in args.module)}
        write_report(record_path, record)
    if args.backend == 'rust' and rust_sources(cargo_extra) != rust_inputs:
        raise ValueError('Rust sources changed during native compilation; refusing to publish the extension')
    suffix = ('-halfclose' if getattr(args, 'fix_halfclose', False) else '') + profile_suffix(args)
    destination = ROOT / 'target/hybrid' / build_alias(args.backend, suffix, dev=args.dev)
    destination.mkdir(parents=True, exist_ok=True)
    switch_link(destination / 'src', source)
    switch_link(destination / 'modules', source / 'modules')
    write_report(destination / 'build-manifest.json', record)
    timings['total'] = round(time.monotonic() - started, 4)
    write_report(destination / 'last-run.json', {'created_utc': datetime.now(timezone.utc).isoformat(),
                 'cache_hit': cached, 'cache_reason': 'all build inputs match' if cached else
                 'incremental make/relink' if args.dev and 'phpize' not in timings else 'fresh native configuration',
                 'timings_seconds': timings, 'build_manifest': str(record_path)})
    print(f'{"Cached" if cached else "Built"} {args.backend}: {destination / "modules/swoole.so"}')
    if getattr(args, 'export', None):
        export_build(record_path, source, args.export, output(['git', 'rev-parse', 'HEAD']))
    print(f'Build phases: {timings}; mode={"development" if args.dev else "immutable"}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
