#!/usr/bin/env python3
"""Build the pinned upstream swoole.so with phpize, configure and make. Never run `make install`.

The default profile builds in the pinned checkout (target/upstream/src); --stdext, --php-sockets and
--configure build a copy of the pin (see tree()). Prints `Built upstream: <swoole.so>`, or
`Cached upstream: <swoole.so>` when that profile's last build had the same configuration."""
import argparse
import json
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


def tree(stdext=False, php_sockets=False, configure=()):
    """Where a profile builds: the pinned checkout itself by default, else a copy of the pin per
    profile (target/upstream-builds/php-sockets-stdext/src and so on), so profiles never rebuild each other."""
    if not (stdext or php_sockets or configure):
        return ROOT / 'target/upstream/src'
    name = '-'.join(name for name, on in [('php-sockets', php_sockets), ('stdext', stdext)] if on) or 'configured'
    return ROOT / 'target/upstream-builds' / name / 'src'


def runner(source, pin):
    """PHPT runs read the pin's tests with phpize's run-tests.php (corpus/upstream/run.py --source),
    which only a build in the pin itself leaves there; a profile's build lends it its own, since
    the runner comes from the PHP (phpize) that profile was built with."""
    if source != pin:
        shutil.copy2(source / 'run-tests.php', pin / 'run-tests.php')


def clone(pin, url='https://github.com/swoole/swoole-src.git'):
    """Clone beside the pin and rename into place: a killed clone leaves only `src.partial`,
    which the next run replaces, never a half-checked-out pin."""
    partial = pin.with_name(pin.name + '.partial')
    shutil.rmtree(partial, ignore_errors=True)
    subprocess.run(['git', 'clone', '--depth', '1', '--branch', VERSION, url, str(partial)], check=True)
    partial.rename(pin)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    add_build_arguments(parser)
    args = parser.parse_args()
    require_build_space(ROOT)
    pin = ROOT / 'target/upstream/src'
    pin.parent.mkdir(parents=True, exist_ok=True)
    if not pin.exists():
        clone(pin)
    actual = subprocess.check_output(['git', '-C', str(pin), 'rev-parse', 'HEAD'], text=True).strip()
    if actual != COMMIT:
        raise RuntimeError(f'Upstream checkout is {actual}; expected {COMMIT}. Refusing to alter it.')
    if subprocess.run(['git', '-C', str(pin), 'diff', '--quiet', 'HEAD', '--']).returncode:
        raise RuntimeError('Upstream has tracked source changes; refusing to build a modified reference.')
    source = tree(args.stdext, args.php_sockets, args.configure)
    library, record_path = source / 'modules/swoole.so', source.parent / 'build.json'
    # The pin never changes, so a build stays current while its configuration does.
    record = {'commit': COMMIT, 'configure': extension_configuration(args)[0], 'platform': [sys.platform, os.uname().machine],
              'php': subprocess.check_output([args.php_config, '--version'], text=True).strip(),
              # build_extension inherits the toolchain from the environment.
              'env': {name: os.environ.get(name) for name in ('CC', 'CXX', 'CFLAGS', 'CPPFLAGS', 'CXXFLAGS', 'LDFLAGS')}}
    if library.is_file() and record_path.is_file() and json.loads(record_path.read_text()) == record:
        runner(source, pin)
        print(f'Cached upstream: {library}')
        return 0
    record_path.unlink(missing_ok=True)
    if source != pin:
        shutil.rmtree(source.parent, ignore_errors=True)
        source.parent.mkdir(parents=True)
        subprocess.run(['git', 'clone', '-q', '--shared', '--no-checkout', str(pin), str(source)], check=True)
        subprocess.run(['git', '-C', str(source), 'checkout', '-q', '--detach', COMMIT], check=True)
    build_extension(source, source.parent / 'build.log', args)
    record_path.write_text(json.dumps(record, indent=1) + '\n')
    runner(source, pin)
    print(f'Built upstream: {library}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
