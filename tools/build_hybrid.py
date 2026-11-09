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
from tools.build_ext import HOOK_SWITCHES, headers_for, native_sources
from tools.check_migration import GRADUATED, MODULES, validate
from tools.harness import ROOT, write_report
from tools.harness import exclusive
from tools.patches import apply_patch_verified, module_patches, patches_for
from tools.private_copy import copy_private

# The integration patch that adds the --with(out)-swoole-rs switch; later patches to
# the same files call into Rust.
BRIDGE_PATCH = 'migration/patches/integration/0001-rust-bridge.patch'
# The build's two Rust archives in rust-prefix/lib, by the input that records each: the
# Zend-free one the native gates link, and the PHP extension's (with LTO unless --dev, #889).
ARCHIVES = {'libswoole_rs_ffi.a': 'rust_archive_sha256', 'libswoole_rs_php.a': 'rust_php_archive_sha256'}


def profile_suffix(args):
    return ('-php-sockets' if getattr(args, 'php_sockets', False) else '') + ('-stdext' if getattr(args, 'stdext', False) else '')


def build_alias(backend, suffix='', *, dev=False):
    """target/hybrid/<alias>: rust or upstream, with the profile's suffix."""
    return ('dev-' if dev else '') + backend + suffix


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
