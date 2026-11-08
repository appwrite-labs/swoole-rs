#!/usr/bin/env python3
"""Build a pinned upstream reference locally. Never run `make install`."""
import argparse
import hashlib
import os
import re
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT
from tools.private_copy import copy_private

VERSION = 'v6.2.5'
COMMIT = '76c657d88be39b7af6bf85e46bcbac0da7387760'


def require_build_space(path):
    """Stop build growth before exhausting the shared host volume; never delete caches."""
    minimum = int(os.environ.get('SWOOLE_RS_MIN_FREE_BYTES', str(4 * 1024**3)))
    if minimum < 0:
        raise ValueError('SWOOLE_RS_MIN_FREE_BYTES must be nonnegative')
    free = shutil.disk_usage(path).free
    if free < minimum:
        raise RuntimeError(f'Build disk space low: {free} bytes free at {path}; require {minimum}. '
                           'Remove only inactive disposable caches; preserve evidence and immutable artifacts.')
    return free


def add_build_arguments(parser):
    parser.add_argument('--jobs', type=int, default=min(os.cpu_count() or 2, 4))
    parser.add_argument('--php-config', default='php-config')
    parser.add_argument('--phpize', default='phpize')
    parser.add_argument('--stdext', action='store_true', help='Explicit experimental upstream stdext profile; separate runtime qualification required')
    parser.add_argument('--php-sockets', action='store_true', help='Enable upstream PHP sockets integration; requires installed php_sockets.h')
    parser.add_argument('--ccache', type=Path, default=shutil.which('ccache'), help='Compiler cache executable; shared bounded cache, detected by default')
    parser.add_argument('--no-ccache', dest='ccache', action='store_const', const=None, help='Disable compiler caching for this build')
    parser.add_argument('--openssl-dir', help='Optional OpenSSL prefix; detected through Homebrew on macOS')
    parser.add_argument('--configure', action='append', default=[], metavar='ARG',
                        help='Extra ./configure argument (write --configure=--enable-x), after the defaults so it overrides them; repeatable')


def extension_configuration(args, extra=()):
    if args.jobs < 1:
        raise ValueError('--jobs must be positive')
    configure = ['./configure', f'--with-php-config={args.php_config}', '--enable-brotli', '--enable-zstd', *extra]
    configure.append('--enable-sockets' if getattr(args, 'php_sockets', False) else '--disable-sockets')
    if getattr(args, 'stdext', False):
        configure.append('--enable-swoole-stdext')
    prefix = args.openssl_dir
    if not prefix and sys.platform == 'darwin':
        prefix = subprocess.check_output(['brew', '--prefix', 'openssl@3'], text=True).strip()
    if prefix:
        configure.append(f'--with-openssl-dir={prefix}')
    configure += getattr(args, 'configure', [])
    env = os.environ.copy()
    includes = subprocess.check_output(['pkg-config', '--cflags', 'libpcre2-8'], text=True).strip()
    env['CPPFLAGS'] = f'{env.get("CPPFLAGS", "")} {includes}'.strip()
    return configure, env


def compiler_cache_identity(args):
    executable = getattr(args, 'ccache', None)
    if not executable:
        return None
    executable = Path(executable).resolve(strict=True)
    return {'path': str(executable),
            'version': subprocess.check_output([str(executable), '--version'], text=True).splitlines()[0]}


def compiler_cache_environment(args, env, log_path):
    identity = compiler_cache_identity(args)
    if identity is None:
        return env
    env = env.copy()
    for variable, compiler in [('CC', 'cc'), ('CXX', 'c++')]:
        command = shlex.split(env.get(variable, compiler))
        if not command:
            raise ValueError(f'{variable} must name a compiler')
        if Path(shutil.which(command[0]) or command[0]).resolve() == Path(identity['path']):
            raise ValueError(f'{variable} already invokes the selected ccache')
        env[variable] = shlex.join([identity['path'], *command])
    return env


def build_extension(source, log_path, args, extra=(), *, incremental=False, timings=None):
    require_build_space(source)
    configure, env = extension_configuration(args, extra)
    env = compiler_cache_environment(args, env, log_path)
    if incremental and any(not Path(name).is_file() for name in re.findall(
            r'^\S+\.lo: (\S+)$', (source / 'Makefile').read_text(), re.MULTILINE)):
        incremental = False  # a source left the build: configure finds the files again
    if incremental:
        for descriptor in source.rglob('*.lo'):
            text = descriptor.read_text(errors='replace')
            objects = re.findall(r"^(?:pic_object|non_pic_object)='([^']+)'$", text, re.MULTILINE)
            if any(name != 'none' and not (descriptor.parent / name).is_file() for name in objects):
                # A libtool descriptor can survive eviction of its underlying
                # object. Make only tracks the descriptor; invalidate it so the
                # normal dependency graph recreates the missing object.
                descriptor.unlink()
    commands = [] if incremental else [('phpize', [args.phpize]), ('configure', configure)]
    # The static Rust archive is a linker flag, not a Makefile dependency: always relink.
    (source / 'swoole.la').unlink(missing_ok=True)
    commands.append(('make', ['make', f'-j{args.jobs}', 'swoole.la']))
    with log_path.open('w') as log:
        for phase, command in commands:
            require_build_space(source)
            print(shlex.join(command), flush=True)
            log.write(shlex.join(command) + '\n')
            log.flush()
            started = time.monotonic()
            result = subprocess.run(command, cwd=source, env=env, stdout=log, stderr=subprocess.STDOUT)
            if timings is not None:
                timings[phase] = round(time.monotonic() - started, 4)
            if result.returncode:
                log.flush()
                # CI keeps no build tree, so the errors must reach stderr.
                errors = [line for line in log_path.read_text(errors='replace').splitlines() if 'error' in line.lower()]
                print('\n'.join(errors[-40:]), file=sys.stderr, flush=True)
                raise RuntimeError(f'Build failed ({result.returncode}). See {log_path}')
            if phase == 'make':
                # GNU cp's sparse-file deallocation fails on some bind mounts.
                # Build the complete library, then publish those exact bytes.
                started = time.monotonic()
                (source / 'modules').mkdir(exist_ok=True)
                copy_private(source / '.libs/swoole.so', source / 'modules/swoole.so')
                log.write('Private clone/copy: .libs/swoole.so -> modules/swoole.so\n')
                if timings is not None:
                    timings['publish_module'] = round(time.monotonic() - started, 4)
    return configure


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_build_arguments(parser)
    args = parser.parse_args()
    require_build_space(ROOT)
    source = ROOT / 'target/upstream/src'
    source.parent.mkdir(parents=True, exist_ok=True)
    if not source.exists():
        subprocess.run(['git', 'clone', '--depth', '1', '--branch', VERSION,
                        'https://github.com/swoole/swoole-src.git', str(source)], check=True)
    actual = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != COMMIT:
        raise RuntimeError(f'Upstream checkout is {actual}; expected {COMMIT}. Refusing to alter it.')
    if subprocess.run(['git', '-C', str(source), 'diff', '--quiet', 'HEAD', '--']).returncode:
        raise RuntimeError('Upstream has tracked source changes; refusing to build a modified reference.')
    build_extension(source, source.parent / 'build.log', args)
    print(f'Upstream {VERSION} ({COMMIT}) built: {source / "modules/swoole.so"}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
