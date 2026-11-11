#!/usr/bin/env python3
"""Short, explicit edit/candidate/integration loops with durable phase timings."""
import argparse
import hashlib
from datetime import datetime, timezone
from fnmatch import fnmatchcase
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import threading
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.annotations import annotate
from tools.harness import ROOT, write_report
from tools.build_upstream import require_build_space, tree
from tools.check_migration import GRADUATED, MODULES
from tools.dev_report import read_json


# Exit 0 only when this Python can configure a TLS 1.3-only client context.
TLS13_PROBE = """import ssl, sys
context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
try:
    context.minimum_version = context.maximum_version = ssl.TLSVersion.TLSv1_3
except ValueError:
    sys.exit(2)
sys.exit(0 if ssl.HAS_TLSv1_3 else 2)"""

# Why a ported core-tests case is excluded.
NETWORK, PROXY = 'needs external DNS and network', 'needs a local HTTP or SOCKS5 proxy service'
# The case's fixed missing name stays in the resolver's negative cache after any lookup of it (the
# other engine's, an earlier run's), and the cached answer comes back before the cancel or timeout.
UNCACHED = 'needs a DNS lookup that misses the resolver cache'
CLIENT_PROXY = ['https_get_async_with_http_proxy', 'https_get_async_with_socks5_proxy', 'https_get_sync_with_http_proxy',
                'https_get_sync_with_socks5_proxy', 'http_get_sync_with_socks5_proxy_no_dns_tunnel', 'http_get_async_with_http_proxy',
                'http_get_async_with_socks5_proxy', 'http_get_sync_with_http_proxy', 'http_get_sync_with_socks5_proxy']
NODE_WS = 'needs Node.js and its ws package'


def require_frozen_source(expected):
    """Boundary guard; individual build/source/artifact attestations remain required."""
    def head():
        return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()
    observed = head()
    if observed != expected:
        raise ValueError('Frozen source HEAD changed: expected ' + expected + ', observed ' + observed)
    dirty = subprocess.check_output(['git', 'status', '--porcelain', '--untracked-files=all'], cwd=ROOT, text=True)
    if dirty.strip() or head() != expected:
        raise ValueError('Frozen source checkout changed; preserve the run and use a clean immutable checkout')


# A change here can affect every module's native build or behaviour.
SHARED_PATHS = ('crates/ffi/', 'crates/ext/', 'native/include/', 'native/swoole-src/', 'native/halfclose/',
                'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/',
                'tools/build_ext.py', 'tools/build_upstream.py', 'migration/modules.json')
# Every PHP class reads and writes zvals through these, so a change here runs every PHPT family,
# though only its consumers' gates (#3044).
ZVAL_PATHS = ('crates/zend/', 'crates/zend-sys/', 'crates/phpswoole/', 'crates/phpstubs/')


def dependents():
    """{crate directory: the directories of the Cargo packages under crates/ and tests/modules/
    whose normal dependencies name it by path (dev-dependencies don't reach the extension)}, relative to ROOT with a trailing slash."""
    import tomllib
    found = {}
    for manifest in [*ROOT.glob('crates/*/Cargo.toml'), *ROOT.glob('tests/modules/**/Cargo.toml')]:
        package = manifest.parent.relative_to(ROOT).as_posix() + '/'
        table = tomllib.loads(manifest.read_text())
        for spec in table.get('dependencies', {}).values():
            if isinstance(spec, dict) and 'path' in spec:
                crate = (manifest.parent / spec['path']).resolve().relative_to(ROOT).as_posix() + '/'
                found.setdefault(crate, set()).add(package)
    return found


def changed_modules(base, consumers=True):
    """Modules whose owned paths the diff from BASE touches, and (with CONSUMERS) the modules
    owning a package that depends on a changed crate: their gates call its exports, so they
    catch one removed or renamed (#2236). Shared build inputs select every module, and so,
    without CONSUMERS (for the PHPT families), do ZVAL_PATHS."""
    names = {module_id: name for name, (module_id, _) in MODULES.items()}
    owners = [(path, names.get(module['id'])) for module in json.loads((ROOT / 'migration/modules.json').read_text())['modules']
              for path in module['owned_paths']]
    owner = lambda path: next((name for owned, name in owners if name and (path == owned or path.startswith(owned))), None)
    users = dependents()
    selected = set()
    for path in subprocess.check_output(['git', 'diff', '--name-only', base + '...HEAD'], cwd=ROOT, text=True).split():
        if not owner(path) and path.startswith(SHARED_PATHS) or not consumers and path.startswith(ZVAL_PATHS):
            return sorted(MODULES)
        selected |= {owner(path)} | {owner(package) for crate, packages in users.items()
                                     if consumers and path.startswith(crate) for package in packages}
    return sorted(selected - {None})


def phpt_families(modules):
    """The upstream PHPT families (tests/<family>/) the named modules own, sorted."""
    ids = {MODULES[module][0] for module in modules}
    return sorted({family for module in json.loads((ROOT / 'migration/modules.json').read_text())['modules']
                   if module['id'] in ids for family in module.get('phpt_families', [])})


def selected_modules(args):
    """The modules whose gates run. Every build has every module; this only narrows the gates."""
    chosen = set(MODULES if getattr(args, 'all_modules', False) else args.module or [])
    return sorted(chosen | set(GRADUATED))


def plan(args, reports):
    modules = selected_modules(args)
    python = sys.executable
    commands = []
    def add(name, *command):
        commands.append((name, list(map(str, command))))
    def script(name, path, *options):
        child = ['--match-child-php'] if getattr(args, 'match_child_php', False) and path == 'corpus/upstream/run.py' else []
        add(name, python, path, *options, *child)
    def report(name):
        return ['--output', str(reports / (name + '.json'))]
    def pytest(name, path, *options):
        add(name, python, '-m', 'pytest', '-q', path, *options, '--junitxml', str(reports / (name + '.xml')))
    php = ['--php', args.php]
    for extension in args.php_extension:
        php += ['--php-extension', extension]
    # Loading an extension PHP has compiled in (tools/asan.py's PHP has curl and sockets) only warns.
    builtin = set(subprocess.run([args.php, '-n', '-m'], capture_output=True, text=True).stdout.lower().split()
                  if sys.platform.startswith('linux') and shutil.which(args.php) else [])
    def php_for(*dependencies):
        return php + [argument for extension in dependencies
                      if sys.platform.startswith('linux') and extension not in args.php_extension and extension not in builtin
                      for argument in ['--php-extension', extension]]
    # A module without a crate (wrappers) has nothing to unit-test.
    crates = Path(__file__).resolve().parents[1] / 'crates'
    # A module's -ffi crate (error-ffi, websocket-ffi) holds the tests of what it exports.
    for module in (m for m in modules if (crates / m / 'Cargo.toml').exists()):
        packages = [arg for crate in (module, module + '-ffi') if (crates / crate / 'Cargo.toml').exists()
                    for arg in ('-p', 'swoole-rs-' + crate)]
        add(module + '-unit', 'cargo', 'test', '--locked', *packages,
            *([args.test] if args.test and args.lane == 'edit' else []))
    if args.lane == 'edit':
        return commands
    integrate = args.lane == 'integrate'
    repair = ['--fix-halfclose'] if getattr(args, 'fix_halfclose', False) else []
    stdext, sockets = getattr(args, 'stdext', False), getattr(args, 'php_sockets', False)
    reference = tree(stdext, sockets) / 'modules/swoole.so'
    # The Cargo-built swoole.so is the engine under test (#2199); `built` is the manifest
    # tools/build_ext.py writes beside it, which gates check their engine against.
    cargo = ROOT / 'target/ext/swoole.so'
    built = cargo.parent / 'build-manifest.json'
    engines = ['--engine', f'cargo={cargo}']
    if integrate or args.compare or args.benchmark:
        engines += ['--engine', f'upstream={reference}']
    if integrate:
        add('format', 'cargo', 'fmt', '--all', '--check')
        add('clippy', 'cargo', 'clippy', '--locked', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings')
    script('ownership', 'tools/check_migration.py')
    if integrate or stdext or sockets:
        script('stdext-reference-build' if stdext else 'reference-build', 'tools/build_upstream.py',
               *(['--stdext'] if stdext else []), *(['--php-sockets'] if sockets else []), '--jobs', args.jobs)
    script('cargo-build', 'tools/build_ext.py', '--native', *repair, *(['--stdext'] if getattr(args, 'stdext', False) else []),
           *(['--php-sockets'] if getattr(args, 'php_sockets', False) else []))
    # The native gates link fixtures against its Zend-free Rust archive and compile against its tree.
    archive = cargo.parent / 'sdk/lib/libswoole_rs_ffi.a'
    script('abi', 'tests/modules/integration/test_bridge.py', *php_for('sockets'), '--extension', cargo, '--reference', reference,
           *(['--archive', archive] if integrate else ['--startup-only']))
    if integrate:
        script('api-surface', 'tools/capability_inventory.py', '--source', ROOT / 'target/upstream/src',
               *php_for('curl', 'sockets', 'tokenizer'), *engines, '--require-same-api', *report('api-surface'))
        # A fixed seed, so a divergence here is a change's; the regression cases run with it.
        script('api-fuzz', 'tests/differential/fuzz.py', *php, '--engine', f'upstream={reference}', '--engine', f'cargo={cargo}',
               '--seed', '0', '--programs', '500', '--jobs', args.jobs, '--cases', ROOT / 'tests/differential/cases',
               *report('api-fuzz'))
        script('zend-bailout', 'tests/modules/zend/run.py', *php_for('curl', 'sockets'), *engines, *report('zend-bailout'))
    if integrate and {'base64', 'crc32'} & set(modules):
        reference_identity = reports / 'reference-identity.json'
        script('reference-identity', 'tools/extension_identity.py', '--source', ROOT / 'target/upstream/src',
               '--extension', reference, *php, '--output', reference_identity)
        for module, runner in [('base64', 'tests/modules/base64/consumers.py'), ('crc32', 'tests/modules/crc32/consumer.py')]:
            if module in modules:
                consumer_php = php_for('curl') if module == 'base64' else php
                for label, extension, identity in [('upstream', reference, reference_identity),
                                                    ('cargo', cargo, built)]:
                    name = module + '-consumers-' + label
                    script(name, runner, '--extension', extension,
                           '--manifest', identity, *consumer_php, *report(name))
    if 'error' in modules:
        script('error-native', 'tests/modules/error/differential.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize', *report('error-native'))
    if 'process' in modules:
        script('process-signals', 'tests/modules/process/signals.py',
               *php, *engines, *report('process-signals'))
    if 'error' in modules and integrate:
        script('error-php', 'tests/modules/error/php.py', '--source', ROOT / 'target/upstream/src',
               *php, *engines, *report('error-php'))
    if 'http' in modules:
        script('http-server-config-owners', 'tests/modules/http/server_config_owners.py',
               '--archive', archive, *report('http-server-config-owners'))
        for name, runner in [('http-rewrites', 'tests/modules/http/rewrite_gate.py'),
                             ('http-locales', 'tests/modules/http/locale_gate.py')]:
            script(name, runner, '--source', ROOT / 'target/upstream/src',
                   '--archive', archive, *report(name))
        pytest('http-smuggling', 'tests/modules/http/test_smuggling.py', *php,
               '--engine', f'upstream={reference}', '--engine', f'cargo={cargo}')
        pytest('http-header-reentry', 'tests/modules/http/test_header_reentry.py', *php, '--engine', f'cargo={cargo}')
    if 'locks' in modules:
        script('locks-native', 'tests/modules/locks/gate.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize', *report('locks-native'))
        script('locks-constructor', 'tests/modules/locks/constructor.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize', *report('locks-constructor'))
        script('locks-platforms', 'tests/modules/locks/platforms.py', '--source', ROOT / 'target/upstream/src', *report('locks-platforms'))
        if integrate:
            script('locks-state', 'tests/modules/locks/state.py', *php_for('curl'), *engines, *report('locks-state'))
    if integrate and 'phpcoroutines' in modules:
        script('phpcoroutines-tracer', 'tests/modules/phpcoroutines/tracer.py', *php, *engines, *report('phpcoroutines-tracer'))
    pytest('buffer-typed', 'tests/modules/buffer/test_typed.py', '--archive', archive, '--sanitize')
    if 'coroutine' in modules:
        # tokens.py runs inside coroutine-channel-state, with and without sanitizers.
        script('coroutine-rust-asan', 'tools/cargo_asan.py', '--package', 'swoole-rs-coroutine', '--report', reports / 'coroutine-rust-asan.json')
    if 'clients' in modules:
        script('clients-http-format', 'tests/modules/clients/http_format_gate.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize',
               *report('clients-http-format'))
        script('clients-php-reentrant-close', 'tests/modules/clients/php_reentrant_close.py', *php_for('curl', 'sockets'),
               '--engine', f'cargo={cargo}', *report('clients-php-reentrant-close'))
        script('clients-php-property-array', 'tests/modules/clients/php_property_array.py', *php_for('curl', 'sockets'),
               '--engine', f'cargo={cargo}', *report('clients-php-property-array'))
        script('clients-php-enable-ssl', 'tests/modules/clients/php_enable_ssl.py', *php_for('curl', 'sockets'),
               '--engine', f'cargo={cargo}', *report('clients-php-enable-ssl'))
        if integrate:
            for family, runner in [('http', 'tests/modules/clients/php_http.py'), ('dns', 'tests/modules/clients/php_dns.py'),
                                   ('lifecycle', 'tests/modules/clients/php_lifecycle.py'),
                                   ('socket-custody', 'tests/modules/clients/socket_custody.py'),
                                   ('pool', 'tests/modules/clients/php_pool.py'), ('resolver', 'tests/modules/clients/php_resolver.py'),
                                   ('pool_fork', 'tests/modules/clients/php_pool_fork.py'),
                                   ('http_connection', 'tests/modules/clients/php_http_connection.py'),
                                   ('http_format', 'tests/modules/clients/php_http_format.py'), ('coro', 'tests/modules/clients/php_coro.py'),
                                   ('async', 'tests/modules/clients/php_async.py'), ('coro_io', 'tests/modules/clients/php_coro_io.py')]:
                name = 'clients-php-' + family
                script(name, runner, '--build-manifest', built,
                       *php_for('curl', 'sockets'), *engines, *report(name))
    if 'phpevents' in modules:
        script('phpevents-lifecycle', 'tests/modules/phpevents/lifecycle.py', *php_for('curl', 'sockets'), *engines,
               *(['--php-sockets'] if getattr(args, 'php_sockets', False) else []), *report('phpevents-lifecycle'))
    if 'phpcollections' in modules:
        if integrate:
            script('phpcollections-runtime', 'tests/modules/phpcollections/php_runtime.py', *php_for('curl'), *engines,
                   '--build-manifest', built, *report('phpcollections-runtime'))
            script('phpcollections-column-bytes', 'tests/modules/phpcollections/php_runtime.py', *php_for('curl'), *engines,
                   '--build-manifest', built, '--profile', 'column-bytes', *report('phpcollections-column-bytes'))
    if 'phphelpers' in modules:
        if integrate:
            script('phphelpers-module', 'tests/modules/phphelpers/module.py', *php_for('curl'), *engines,
                   '--build-manifest', built, *report('phphelpers-module'))
            # The fixtures call stdext helpers (swoole_typed_array), so they need --stdext builds.
            if getattr(args, 'stdext', False):
                script('phphelpers-runtime', 'tests/modules/phphelpers/runtime.py', *php, *engines,
                       '--build-manifest', built, *report('phphelpers-runtime'))
    if 'ports' in modules:
        # SNI resolves its port inside the callback; exercise the production wrapper,
        # not only the protocol-plan and scalar-setting mini gates.
        sni_engines = engines if integrate or args.compare else ['--engine', f'cargo={cargo}']
        script('ports-sni-originals', 'corpus/upstream/run.py',
               '--manifest', 'tests/modules/ports/sni-originals.json', *php_for('curl', 'sockets'), *sni_engines,
               '--require-all', '--isolate-cases', *report('ports-sni-originals'))
        if integrate:
            script('ports-configuration', 'tests/modules/ports/configuration.py', *php_for('curl', 'sockets'), *engines,
                   '--build-manifest', built, *report('ports-configuration'))
        script('ports-policy', 'tests/modules/ports/gate.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, *report('ports-policy'))
        phase = 'ports-callback-property'
        script(phase, 'tests/modules/ports/callback_property.py', '--source', ROOT / 'target/upstream/src', '--sanitize',
               '--archive', archive, *report(phase))
    if 'corebase' in modules:
        script('corebase-dns-request-guard', 'tests/modules/corebase/resolver_guard.py',
               '--archive', archive, *report('corebase-dns-request-guard'))
        script('corebase-record-views', 'tests/modules/corebase/record_views.py',
               '--archive', archive, *report('corebase-record-views'))
        script('corebase-constants', 'tests/modules/corebase/constants.py', '--source', ROOT / 'target/upstream/src', *report('corebase-constants'))
        includes = (['--include', str(Path(subprocess.check_output(['brew', '--prefix', 'openssl@3'], text=True).strip()) / 'include')]
                    if sys.platform == 'darwin' else [])
        script('corebase-native', 'tests/modules/corebase/differential.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize', *includes, *report('corebase-native'))
        if integrate:
            script('corebase-startup', 'tests/modules/corebase/mkdir_startup.py', *php, *engines,
                   '--build-manifest', built, *report('corebase-startup'))
    if 'tls' in modules:
        script('tls-constants', 'tests/modules/corebase/constants.py', '--source', ROOT / 'target/upstream/src',
               '--include-tls', *report('tls-constants'))
    if 'hooks' in modules and sys.platform.startswith('linux'):
        # core-tests/src/coroutine/{system,hook,socket,base,file,async}.cpp, network/{client,dns,address,socket}.cpp,
        # os/{timer,pipe,process_pool,wait,async}.cpp, os.daemon and protocol/ssl.cpp, ported to Rust over the Cargo build's libswoole, and the repo-owned C++
        # witnesses in its fixture.
        script('hooks-system-rust', 'tests/modules/hooks/system.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive,
               '--fixture', ROOT / 'tests/modules/hooks/system/fixture.cc',
               '--build-root', ROOT / 'target/native-originals/hooks-system', '--jobs', args.jobs,
               '--exclude', 'coroutine_system.getaddrinfo=' + NETWORK, '--exclude', 'coroutine_system.getaddrinfo_timeout=' + UNCACHED,
               '--exclude', "coroutine_hook.sendmsg_and_recvmsg=its child reads the parent's freed stack when the parent's close "
                            'completes first (SIGSEGV in ~15% of runs upstream)',
               '--exclude', 'coroutine_socket.tcp6=needs IPv6',
               '--exclude', 'coroutine_async.gethostbyname=' + NETWORK,
               *[arg for name, reason in [('lookup1', NETWORK), ('lookup_ipv6', NETWORK), ('bad_family', NETWORK), ('cancel', UNCACHED)]
                 for arg in ['--exclude', f'dns.{name}={reason}']],
               # core-tests/src/coroutine/gethostbyname.cpp (system/src/gethostbyname.rs) resolves public names.
               *[arg for name in ['resolve_cache', 'resolve_cache_inet4_and_inet6', 'dns_expire']
                 for arg in ['--exclude', f'coroutine_gethostbyname.{name}={NETWORK}']],
               *[arg for name in ['https_get_with_socks5_proxy', 'http_get_with_socks5_proxy', 'http_get_with_http_proxy',
                                  'https_get_with_http_proxy'] for arg in ['--exclude', f'coroutine_socket.{name}={PROXY}']],
               # core-tests/src/network/client.cpp (tests/modules/clients/client.rs).
               '--exclude', "client.bind=expects EINVAL binding port 80 under GITHUB_ACTIONS (a root container); the runner "
                            "isn't root and gets EACCES",
               '--exclude', 'client.async_tcp_socks5_proxy_handshake_fail=expects www.baidu.com:80 to time out a SOCKS5 greeting',
               '--exclude', 'client.async_tcp_ssl_handshake_fail=' + NETWORK + ': its TLS handshake with www.baidu.com:80 '
                            'has a 1 s timeout the round trip sometimes exceeds',
               '--exclude', "client.sync_ssl_sendfile=each recv lands at the buffer's start, so past the largest TLS record "
                            "it reads bytes no recv wrote (uninitialised upstream, zeroed here); www.baidu.com's ~730 KB "
                            'page leaves "baidu.com" only in overwritten bytes',
               *[arg for name in CLIENT_PROXY for arg in ['--exclude', f'client.{name}={PROXY}']],
               # core-tests/src/network/socket.cpp (system/src/network_socket.rs).
               '--exclude', "socket.make_server_socket=expects ESOCKTNOSUPPORT for an unprivileged raw socket; the runner's "
                            'kernel returns EPROTONOSUPPORT',
               '--exclude', 'socket.connect_sync=' + NETWORK, '--exclude', 'socket.fail=' + NETWORK,
               *report('hooks-system-rust'))
    if 'reactor' in modules and sys.platform.startswith('linux'):
        # core-tests/src/reactor/base.cpp, ported to Rust over the Cargo build's libswoole.
        script('reactor-base-rust', 'tests/modules/hooks/system.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive,
               '--build-root', ROOT / 'target/native-originals/reactor-base', '--jobs', args.jobs,
               '--package', ROOT / 'tests/modules/reactor/base', '--fixture', ROOT / 'tests/modules/reactor/base/fixture.cc',
               *report('reactor-base-rust'))
    if 'http' in modules and sys.platform.startswith('linux'):
        # core-tests/src/server/http_server.cpp's, protocol/base.cpp's and redis.cpp's and server/mqtt.cpp's server
        # TESTs, ported to Rust over the Cargo build's libswoole.
        script('http-server-rust', 'tests/modules/hooks/system.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive,
               '--build-root', ROOT / 'target/native-originals/http-server', '--jobs', args.jobs,
               '--package', ROOT / 'tests/modules/http/server',
               '--exclude', 'http_server.node_websocket_client_1=' + NODE_WS, '--exclude', 'http_server.node_websocket_client_2=' + NODE_WS,
               '--exclude', 'redis.get=needs a Redis server on 127.0.0.1:6379', '--exclude', 'mqtt.echo=needs Node.js and its mqtt and pino packages',
               *report('http-server-rust'))
    if 'utilities' in modules and sys.platform.startswith('linux'):  # Rust implements only the futex path; others run upstream's registry.
        script('utilities-wait', 'tests/modules/utilities/wait.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize', *report('utilities-wait'))
    if 'utilities' in modules and integrate:
        for family in ['logger', 'hash']:
            name = 'utilities-' + family + '-originals'
            script(name, 'tests/modules/utilities/storage.py', '--source', ROOT / 'target/upstream/src',
                   *php_for('curl'), *engines, '--build-manifest', built, '--archive', archive,
                   '--' + family + '-originals', *report(name))
        script('utilities-php-logger', 'tests/modules/utilities/php_logger.py', *php, *engines,
               '--build-manifest', built, *report('utilities-php-logger'))
        script('utilities-public-storage', 'tests/modules/utilities/storage.py', '--source', ROOT / 'target/upstream/src',
               *php, *engines, '--build-manifest', built, '--archive', archive, *report('utilities-public-storage'))
    if 'memory' in modules:
        script('memory-rust-asan', 'tools/cargo_asan.py', '--package', 'swoole-rs-memory', '--report', reports / 'memory-rust-asan.json')
    if 'memory' in modules and integrate:
        script('memory-table-witnesses', 'tests/modules/memory/witnesses.py', *php, *engines,
               '--require-memory-bridge', 'cargo', *report('memory-table-witnesses'))
    if 'protocols' in modules:
        script('protocols-native', 'tests/modules/protocols/native.py', '--source', ROOT / 'target/upstream/src',
               '--archive', archive, '--sanitize',
               *report('protocols-native'))
        script('protocols-network', 'tests/modules/protocols/network.py', *php, *engines, *report('protocols-network'))
        if integrate:
            reference_first = ['--engine', f'upstream={reference}', '--engine', f'cargo={cargo}']
            for runner in ['tests/modules/protocols/reply.py', 'tests/modules/protocols/request.py', 'tests/modules/protocols/handlers.py',
                           'tests/modules/protocols/bailout.py', 'tests/modules/protocols/lifecycle.py']:
                name = 'protocols-' + Path(runner).stem
                script(name, runner, *php, *reference_first, *report(name))
    if 'timer' in modules:
        pytest('timer-fork-active', 'tests/modules/timer/test_fork.py', *php, *engines)
        if integrate:
            pytest('timer-bailout', 'tests/modules/timer/test_bailout.py', *php,
                   '--engine', f'upstream={reference}', '--engine', f'cargo={cargo}')
    if 'buffer' in modules:
        pytest('buffer-string-compatibility', 'tests/modules/buffer/string/test_compatibility.py',
               '--archive', archive, '--sanitize')
        if integrate and getattr(args, 'php_sockets', False):
            script('php-sockets-sendfile-original', 'corpus/upstream/run.py',
                   '--manifest', ROOT / 'tests/modules/buffer/originals.json',
                   '--case', 'tests/swoole_server/sendfile_02.phpt', *php_for('curl', 'sockets'),
                   *engines, '--require-all', '--isolate-cases', *report('php-sockets-sendfile-original'))
        pytest('buffer-posix-iovec', 'tests/modules/buffer/test_iovec.py', '--archive', archive, '--sanitize')
        pytest('string-originals', 'tests/modules/buffer/string/test_gate.py',
               '--archive', archive, '--sanitize')
        pytest('buffer-network', 'tests/modules/buffer/test_network.py', *php, *engines)
    if 'http2' in modules:
        pytest('http2-frames', 'tests/modules/http2', *php, '--engine', f'cargo={cargo}')
    if 'websocket' in modules:
        pytest('websocket-codec', 'tests/modules/websocket/test_codec.py', '--archive', archive, '--sanitize')
        pytest('websocket-differential', 'tests/modules/websocket/test_differential.py', *php,
               '--engine', f'upstream={reference}', '--engine', f'cargo={cargo}')
        pytest('websocket-invalid', 'tests/modules/websocket/test_invalid.py', *php, *engines)
        if integrate:
            pytest('websocket-close', 'tests/modules/websocket/test_close.py', *php, *engines)
    if repair:
        tls13_python = getattr(args, 'tls13_python', None) or python
        add('halfclose-tls13-driver', tls13_python, '-I', '-S', '-B', '-c', TLS13_PROBE)
        candidate = ['--engine', f'cargo={cargo}']
        script('halfclose-queued', 'tests/modules/halfclose/gate.py', *php, *candidate, *report('halfclose-queued'))
        script('halfclose-prequeue', 'tests/modules/halfclose/prequeue.py', *php, *candidate, *report('halfclose-prequeue'))
        script('halfclose-pressure', 'tests/modules/halfclose/prequeue.py', *php, *candidate,
               '--workers', '2', '--keep-alive', '--max-queued-bytes', '1', *report('halfclose-pressure'))
        for coroutines in [False, True]:
            name = 'halfclose-paused' + ('-coroutines' if coroutines else '')
            profile = ['--coroutines'] if coroutines else []
            script(name, 'tests/modules/halfclose/pause.py', *php, *candidate, *profile, *report(name))
            certs = ROOT / 'target/upstream/src/tests/include/ssl_certs'
            for version in ['1.2', '1.3']:
                tls_name = name + '-tls' + version
                add(tls_name, tls13_python if version == '1.3' else python, 'tests/modules/halfclose/pause.py', *php, *candidate, *profile,
                       '--tls-cert', certs / 'server.crt', '--tls-key', certs / 'server.key',
                       '--tls-version', version, *report(tls_name))
        script('halfclose-pause-originals', 'corpus/upstream/run.py', *php, *candidate,
               '--manifest', 'tests/modules/halfclose/pause-originals.json', '--require-all',
               *report('halfclose-pause-originals'))
    if integrate:
        # 11-buffer-lifecycle hits upstream's half-close truncation, which only --fix-halfclose
        # repairs, so it fails on both engines otherwise. The PR tier runs it on a repaired build.
        projects = [] if getattr(args, 'fix_halfclose', False) else [
            arg for project in json.loads((ROOT / 'corpus/manifest.json').read_text())
            if project['id'] != '11-buffer-lifecycle' for arg in ['--project', project['id']]]
        script('applications', 'corpus/run.py', *php, *engines, *projects, '--require-all', *report('applications'))
    if sys.platform.startswith('linux') and (integrate or 'reactor' in modules):
        # The applications under strace: the socket options, flags and epoll registrations
        # PHP can't see. Seconds, so the candidate lane runs it for the reactor's sockets too.
        # The PR tier doesn't run hyperf.py --prepare, so the candidate lane leaves Hyperf out.
        projects = [] if integrate else [arg for project in json.loads((ROOT / 'corpus/manifest.json').read_text())
                                         if project['id'] != '12-hyperf-skeleton' for arg in ['--project', project['id']]]
        script('syscalls', 'corpus/syscalls.py', *php, '--engine', f'upstream={reference}', '--engine', f'cargo={cargo}',
               *projects, *report('syscalls'))
    network = ['--external-network'] if getattr(args, 'external_network', False) else []
    if args.case and not integrate:
        script('phpt', 'corpus/upstream/run.py', *php, *engines, '--jobs', args.jobs,
               *[arg for case in args.case for arg in ['--case', case]], '--require-all', *network, *report('phpt'))
    elif (not integrate and not getattr(args, 'all_modules', False) and set(args.module or ['websocket']) < set(MODULES)
          and getattr(args, 'phpt_module', None) != sorted(MODULES)):
        # The original PHPT families of the chosen modules, gated on upstream like the merge
        # tier's phpt jobs, which run every family (and so cover a change to all modules or ZVAL_PATHS).
        # Only the modules a change touches: a consumer's families can't change outcome
        # without its own code changing, and every family together overruns the phase cap.
        # An empty list (only consumers changed) means no families, not a fallback.
        touched = getattr(args, 'phpt_module', None)
        families = phpt_families(args.module or ['websocket'] if touched is None else touched)
        if families:
            add('phpt', 'corpus/upstream/families.sh', reports / 'phpt', reference, cargo, *php,
                *[arg for family in families for arg in ['--module', family]], *network)
    if args.benchmark:
        # These phases are strictly after correctness checks and never concurrent.
        if 'timer' in modules:
            script('timer-benchmark', 'tests/modules/timer/benchmark.py', *php, *engines, *report('timer-benchmark'))
        if 'buffer' in modules:
            for peek in [False, True]:
                name = 'buffer-benchmark' + ('-consumer' if peek else '')
                script(name, 'tests/modules/buffer/benchmark.py', '--archive', archive,
                       *(['--peek-front'] if peek else []), *report(name))
        if 'websocket' in modules:
            script('websocket-benchmark', 'tests/modules/websocket/benchmark.py', *php, *engines, *report('websocket-benchmark'))
    return commands




# Phases that build artifacts later phases consume, or that later phases need to have
# passed. Each runs alone, after everything before it and before everything after it.
BARRIER_PHASES = {'ownership', 'cargo-build', 'reference-build', 'stdext-reference-build'}


def serial_phase(name):
    """Phases that can't share the machine: fixed ports (the PHPT runners), timing, benchmarks."""
    return (name == 'phpt' or name.endswith('-originals') or 'benchmark' in name or 'timing' in name)


def phase_dependencies(commands, reports):
    """A phase depends on every earlier barrier and on any earlier phase whose report it reads."""
    owners = {str(reports / (name + suffix)): name for name, _ in commands for suffix in ('.json', '.xml')}
    dependencies, barriers = {}, set()
    for name, command in commands:
        produced = {owners[str(arg)] for arg in command if str(arg) in owners} - {name}
        earlier = set(dependencies)
        dependencies[name] = (earlier if name in BARRIER_PHASES else barriers) | (produced & earlier)
        if name in BARRIER_PHASES:
            barriers.add(name)
    return dependencies


LOG_TAIL = 30  # lines of a failed phase's log kept in loop.json
DISK_REFUSAL = re.compile(r'Build disk space low: [^"\n]*')
PHASE_WEIGHTS = ROOT / 'tools/phase_weights.json'  # seconds per phase from a merge-tier run
DEFAULT_WEIGHT = 2
SHARD_CORES = 4  # a merge-tier runner's vCPUs


def shard(commands, reports, index, count):
    """Every barrier (and what barriers read), plus the index-th of count groups of the other
    phases, balanced by weight. A phase always shares a shard with phases whose report it reads.
    A group whose phases read each other in sequence weighs at least that chain on every core,
    since nothing on its shard finishes before the chain does (#318)."""
    weights = json.loads(PHASE_WEIGHTS.read_text())
    owners = {str(reports / (name + suffix)): name for name, _ in commands for suffix in ('.json', '.xml')}
    reads = {name: {owners[str(arg)] for arg in command if str(arg) in owners} - {name} for name, command in commands}
    everywhere = set(BARRIER_PHASES)
    for name in [n for n, _ in commands if n in BARRIER_PHASES]:
        stack = list(reads[name])
        while stack:
            other = stack.pop()
            if other not in everywhere:
                everywhere.add(other)
                stack += reads[other]
    group = {name: name for name, _ in commands}
    def root(name):
        while group[name] != name:
            name = group[name]
        return name
    for name, needs in reads.items():
        for other in needs - everywhere:
            group[root(other)] = root(name)
    chains = {}
    def chain(name):
        if name not in chains:
            chains[name] = weights.get(name, DEFAULT_WEIGHT) + max(
                (chain(other) for other in reads[name] - everywhere), default=0)
        return chains[name]
    sizes, longest = {}, {}
    for name, _ in commands:
        if name not in everywhere:
            sizes[root(name)] = sizes.get(root(name), 0) + weights.get(name, DEFAULT_WEIGHT)
            longest[root(name)] = max(longest.get(root(name), 0), chain(name))
    sizes = {unit: max(size, SHARD_CORES * longest[unit]) for unit, size in sizes.items()}
    load, slot = [0] * count, {}
    for unit in sorted(sizes, key=lambda unit: (-sizes[unit], unit)):
        slot[unit] = load.index(min(load))
        load[slot[unit]] += sizes[unit]
    return [(name, command) for name, command in commands if name in everywhere or slot[root(name)] == index - 1]


# The pinned reference: rebuilt rarely.
PRUNE_KEEP = {'upstream'}


def prune(root, older_than_hours):
    """Delete per-build output under target/ untouched for the window; return (paths, bytes) freed."""
    cutoff = time.time() - older_than_hours * 3600
    target = root / 'target'
    candidates = []
    for directory, keep in [(target, PRUNE_KEEP | {'worktrees'}), (target / 'worktrees', set())]:
        if directory.is_dir():
            candidates += [path for path in directory.iterdir() if path.name not in keep]
    freed, removed = 0, []
    for path in candidates:
        if path.lstat().st_mtime >= cutoff:
            continue
        if path.is_dir() and not path.is_symlink():
            freed += sum(f.lstat().st_size for f in path.rglob('*') if not f.is_dir() or f.is_symlink())
            shutil.rmtree(path, ignore_errors=True)
        else:
            freed += path.lstat().st_size
            path.unlink()
        removed.append(path)
    if any(path.parent == target / 'worktrees' for path in removed):
        subprocess.run(['git', 'worktree', 'prune'], cwd=root, check=False)
    return removed, freed


DOCKER_REPOSITORY = 'swoole-rs-dev'


def docker_image():
    """The dev image for this host's architecture and the current .devcontainer contents."""
    digest = hashlib.sha256()
    for path in sorted((ROOT / '.devcontainer').rglob('*')):
        if path.is_file():
            digest.update(path.relative_to(ROOT).as_posix().encode() + b'\0' + path.read_bytes())
    return f'{DOCKER_REPOSITORY}:{os.uname().machine}-{digest.hexdigest()[:12]}'


# Inside the container: the pinned toolchain, the locked crates and the pinned upstream
# build, then dev.py with the image's PHP extensions unless the command names its own.
DOCKER_SCRIPT = r'''set -e
git config --global --add safe.directory '*'
# Entries of one worktree and architecture share target/upstream: prepare it one at a time.
{
flock 9
rustup install >/dev/null
cargo fetch --locked -q
python3 tools/build_upstream.py > target/upstream/build-upstream.log 2>&1 || { tail -20 target/upstream/build-upstream.log; exit 1; }
} 9>target/upstream/prepare.lock
# A repository script (benchmarks/callgrind.py, a gate) runs as itself.
case "$1" in *.py) exec python3 "$@" ;; esac
extensions=
case " $* " in *" --php-extension "*) ;; *" candidate "*|*" integrate "*)
    for name in $(sed -n 's/^extension=\(.*\)/\1/p' /etc/php/*/cli/conf.d/*.ini | sed 's/\.so$//' | sort -u); do
        extensions="$extensions --php-extension $name"
    done ;;
esac
exec python3 tools/dev.py "$@" $extensions
'''


def docker(arguments):
    """dev.py docker [-- dev.py arguments | -- SCRIPT.py arguments]: run a dev.py command, or a
    repository script, in the Linux dev image."""
    if arguments[:1] == ['--']:
        arguments = arguments[1:]
    if not arguments:
        sys.exit('usage: dev.py docker [--] LANE [dev.py options] | [--] SCRIPT.py [arguments]')
    if '--phase' in arguments and '--with-prerequisites' not in arguments:
        arguments = [*arguments, '--with-prerequisites']
    free = shutil.disk_usage(ROOT).free
    if free < DOCKER_MIN_FREE:
        print(f'dev.py docker: only {free / 1e9:.1f} GB free; `python3 tools/dev.py prune` clears old builds',
              file=sys.stderr)
    image = docker_image()
    if subprocess.run(['docker', 'image', 'inspect', image], capture_output=True).returncode:
        subprocess.run(['docker', 'build', '-t', image, str(ROOT / '.devcontainer')], check=True)
    common = Path(subprocess.check_output(['git', 'rev-parse', '--path-format=absolute', '--git-common-dir'],
                                          cwd=ROOT, text=True).strip())
    architecture = os.uname().machine
    mounts = [ROOT, common] if not common.is_relative_to(ROOT) else [ROOT]
    # Linux builds live in target/docker/<arch>/, seen inside at their usual paths, so
    # the host's own target/upstream and target/ext never hold another platform's.
    private = ROOT / 'target/docker' / architecture
    builds = []
    for name in ['upstream', 'upstream-builds', 'ext']:
        (private / name).mkdir(parents=True, exist_ok=True)
        builds += ['-v', f'{private / name}:{ROOT / "target" / name}']
    command = ['docker', 'run', '--rm', '--init', '-w', str(ROOT),
               *[part for mount in mounts for part in ('-v', f'{mount}:{mount}')], *builds,
               '-v', f'swr-rustup-{architecture}:/usr/local/rustup',
               '-v', f'swr-cargo-{architecture}:/usr/local/cargo/registry',
               '-e', f'CARGO_TARGET_DIR={private}/cargo',
               image, 'bash', '-c', DOCKER_SCRIPT, 'dev.py', *arguments]
    return subprocess.run(command).returncode


DOCKER_MIN_FREE = 20 * 1024**3


def with_prerequisites(planned, phases):
    """`phases` plus, transitively, the phases whose reports they read and the builds whose
    output they use, in plan order."""
    names = [name for name, _ in planned]
    commands = {name: list(map(str, command)) for name, command in planned}
    selected = set(phases)
    while True:
        grown = set(selected)
        for name in selected:
            for argument in commands.get(name, []):
                grown |= {owner for owner in names if owner != name and argument.endswith('/' + owner + '.json')}
                reference = '/target/upstream/' in argument or '/target/upstream-builds/' in argument
                if 'cargo-build' in names and '/target/ext/' in argument:
                    grown.add('cargo-build')
                grown |= {owner for owner in ('reference-build', 'stdext-reference-build') if owner in names and reference}
        if grown == selected:
            return [name for name in names if name in selected]
        selected = grown


def shard_spec(value):
    index, _, count = value.partition('/')
    if not (index.isdigit() and count.isdigit() and 1 <= int(index) <= int(count)):
        raise argparse.ArgumentTypeError('expected I/N with 1 <= I <= N')
    return int(index), int(count)


def execute(command, *, timeout, **options):
    """subprocess.run in its own process group; on timeout the whole group (servers a phase
    started included) is killed and returncode is None."""
    process = subprocess.Popen(command, start_new_session=True, **options)
    try:
        return subprocess.CompletedProcess(command, process.wait(timeout=timeout))
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        return subprocess.CompletedProcess(command, None)


def schedule(commands, dependencies):
    """Plan order, except that serial phases nothing reads from move to the end of their
    barrier segment, so they don't drain the pipeline in the middle of parallel work."""
    needed = set().union(set(), *(d for n, d in dependencies.items() if n not in BARRIER_PHASES))
    ordered, deferred = [], []
    for phase in commands:
        name = phase[0]
        if name in BARRIER_PHASES:
            ordered += deferred
            deferred = []
            ordered.append(phase)
        elif serial_phase(name) and name not in needed:
            deferred.append(phase)
        else:
            ordered.append(phase)
    return ordered + deferred


def main():
    if sys.argv[1:2] == ['docker']:
        return docker(sys.argv[2:])
    parser = argparse.ArgumentParser(description=__doc__, epilog=docker.__doc__)
    parser.add_argument('lane', choices=['edit', 'candidate', 'integrate', 'prune'])
    parser.add_argument('--older-than', type=float, default=24, metavar='HOURS',
                        help='prune: delete target/ build output untouched for this long')
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument('--module', choices=sorted(MODULES), action='append', help="Run this module's gates (graduated modules' always run)")
    selection.add_argument('--all-modules', action='store_true', help="Run every module's gates")
    selection.add_argument('--changed-since', metavar='REF', help='Run the gates of the modules a diff from REF touches (all for shared build inputs)')
    parser.add_argument('--stdext', action='store_true', help='Build and compare genuine enabled experimental stdext profiles')
    parser.add_argument('--php-sockets', action='store_true', help='Build matching Native/Rust PHP sockets-integration profiles')
    parser.add_argument('--external-network', action='store_true',
                        help='Also run original PHPTs that need internet access; without it they stay unmeasured')
    parser.add_argument('--fix-halfclose', action='store_true', help='Select the separate native queued-close drain repair')
    parser.add_argument('--test', help='Rust test-name filter (edit lane only)')
    parser.add_argument('--case', action='append', help='Exact registered PHPT path (candidate lane only)')
    parser.add_argument('--jobs', type=int, default=os.cpu_count() or 4,
                        help='Phases run at once, and parallelism passed to builds (default: CPU count)')
    parser.add_argument('--php', default='php')
    parser.add_argument('--match-child-php', action='store_true',
                        help='Bind bare php children in direct official-runner phases to their selected parent; default off')
    parser.add_argument('--tls13-python', type=Path, help='Explicit Python SSL provider for native-repair TLS1.3 clients only; defaults to this driver, without fallback')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--hypothesis', help='What this experiment should establish')
    parser.add_argument('--expect', help='Observable outcome that supports the hypothesis')
    parser.add_argument('--benchmark', action='store_true', help='Run isolated benchmarks after all correctness phases pass')
    parser.add_argument('--compare', action='store_true', help='Also rerun upstream controls in the candidate lane (integration always does)')
    parser.add_argument('--phase', action='append', help='Run named canonical phases (or fnmatch patterns) only; omitted gates remain explicitly not run and cannot qualify full integration')
    parser.add_argument('--skip-phase', action='append', default=[], metavar='PATTERN',
                        help='Leave out the planned phases matching this fnmatch pattern; they remain explicitly not run')
    parser.add_argument('--with-prerequisites', action='store_true',
                        help='Add the phases each --phase needs: the reports it reads and the builds it uses')
    parser.add_argument('--phase-timeout', type=int, default=1200, metavar='SECONDS',
                        help='Kill a phase (and the processes it started) after this long; it fails')
    parser.add_argument('--shard', type=shard_spec, metavar='I/N',
                        help='Run slice I of N of the integrate plan; every slice builds and runs the barriers')
    parser.add_argument('--frozen-source', help='Require this literal 40-hex HEAD and a clean checkout before and after each integration phase')
    parser.add_argument('--plan', action='store_true', help='Print commands without executing or writing reports')
    args = parser.parse_args()
    if args.lane == 'prune':
        removed, freed = prune(ROOT, args.older_than)
        if removed:
            print('dev.py prune: freed %.1f GB in %d entries under target/' % (freed / 1e9, len(removed)))
        return 0
    if args.changed_since:
        args.module = changed_modules(args.changed_since)
        args.phpt_module = changed_modules(args.changed_since, consumers=False)
        if not args.module:
            print('No module touched since ' + args.changed_since)
            return 0
        print('Modules touched since ' + args.changed_since + ': ' + ', '.join(args.module))
    if args.frozen_source and (args.lane != 'integrate' or len(args.frozen_source) != 40
                               or any(c not in '0123456789abcdef' for c in args.frozen_source)):
        parser.error('--frozen-source requires integration and a literal lowercase 40-hex commit')
    if args.jobs < 1 or args.test and args.lane != 'edit' or args.case and args.lane != 'candidate':
        parser.error('Positive --jobs required; --test is edit-only and --case is candidate-only')
    if args.benchmark and args.lane == 'edit':
        parser.error('Benchmarks require candidate or integrate')
    if args.phase and (args.lane == 'edit' or args.benchmark):
        parser.error('--phase requires candidate/integrate and cannot qualify a benchmark run')
    if args.shard and (args.lane != 'integrate' or args.phase):
        parser.error('--shard splits the full integrate plan; it cannot be combined with --phase')
    stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S') + '-' + uuid.uuid4().hex[:8]
    reports = ROOT / 'target/reports/dev' / stamp
    planned = plan(args, reports)
    if args.phase:
        # A pattern (`--phase 'halfclose-*'`) names every planned phase it matches.
        args.phase = [match for pattern in args.phase
                      for match in ([name for name, _ in planned if fnmatchcase(name, pattern)] or [pattern])]
    if args.phase and args.with_prerequisites:
        args.phase = with_prerequisites(planned, args.phase)
    if args.phase:
        unknown = set(args.phase) - {name for name, _ in planned}
        if unknown:
            parser.error('Unknown phases: ' + ', '.join(sorted(unknown)))
        report_owners = {str(reports / (name + '.json')): name for name, _ in planned}
        for name, command in planned:
            if name in args.phase:
                missing = {report_owners[arg] for arg in command if arg in report_owners} - set(args.phase) - {name}
                if missing:
                    parser.error('--phase ' + name + ' requires report-producing phases in the same run: '
                                 + ', '.join(sorted(missing)))
    commands = [(name, command) for name, command in planned if (not args.phase or name in args.phase)
                and not any(fnmatchcase(name, pattern) for pattern in args.skip_phase)]
    if args.shard:
        commands = shard(commands, reports, *args.shard)
    if args.plan:
        print(json.dumps([{'phase': name, 'command': command} for name, command in commands], indent=2))
        return 0
    reports.mkdir(parents=True)
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'lane': args.lane,
              'hypothesis': args.hypothesis, 'expected_observation': args.expect,
              'match_child_php_requested': args.match_child_php, 'frozen_source_required': args.frozen_source,
              'phases': [], 'status': 'running', 'selection_scope': 'explicit_phase_subset' if args.phase or args.skip_phase else 'complete_plan',
              'planned_phases': [name for name, _ in planned], 'selected_phases': [name for name, _ in commands],
              'full_plan_passed': False,
              'driver_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    environment = dict(os.environ)
    def frozen_error():
        if args.frozen_source:
            try:
                require_frozen_source(args.frozen_source)
            except (OSError, ValueError, subprocess.CalledProcessError) as error:
                return str(error)
        return None

    started = time.monotonic()
    order = {name: index for index, (name, _) in enumerate(commands)}
    dependencies = phase_dependencies(commands, reports)
    lock = threading.Condition()
    running, finished = set(), set()

    def run_phase(name, command):
        print(f'[{name}] {shlex.join(command)}', flush=True)
        phase_started = time.monotonic()
        with (reports / (name + '.log')).open('w') as log:
            result = execute(command, timeout=args.phase_timeout, cwd=ROOT, env=environment, stdout=log,
                             stderr=subprocess.STDOUT)
            if result.returncode is None:
                log.write(f'\ndev.py: phase timed out after {args.phase_timeout}s and was killed\n')
        row = {'phase': name, 'command': command, 'seconds': round(time.monotonic() - phase_started, 4),
               'returncode': result.returncode, 'failed': result.returncode != 0, 'log': str(reports / (name + '.log'))}
        if result.returncode is None:
            row['timed_out'] = True
        outcome_flag = next((flag for flag in ('--output', '--report') if flag in command), None)
        if outcome_flag:
            path = Path(command[command.index(outcome_flag) + 1])
            if not path.is_absolute():
                path = ROOT / path
            try:
                outcome, row['outcome_sha256'] = read_json(path, with_digest=True)
                if 'passed' in outcome:
                    row['outcome_passed'] = outcome['passed']
                    if not isinstance(outcome['passed'], bool):
                        raise ValueError('Gate passed field must be boolean')
                    row['failed'] |= not outcome['passed']
                if 'compile' in outcome:
                    row['compile'] = outcome['compile']
            except (OSError, ValueError) as error:
                row.update(failed=True, outcome_error=str(error))
        # A gate's require_build_space refusal (tools/build_upstream.py), whether it reports the
        # refusal or dies of it, stops the run like dev.py's own.
        if row['failed'] and (refusal := DISK_REFUSAL.search(Path(row['log']).read_text(errors='replace'))):
            row['infrastructure_error'] = refusal[0]
        print(f'  [{name}] {row["seconds"]:.3f}s; exit={result.returncode}; failed={row["failed"]}; {row["log"]}', flush=True)
        if row['failed']:
            annotate('error', f'{name} failed', f'exit {result.returncode}, {row["log"]}')
        return row

    def record(row):
        # Called with `lock` held. Rows stay in plan order whatever order phases finish in.
        name = row['phase']
        error = row.pop('infrastructure_error', None)
        source = frozen_error()
        if source:
            row.update(failed=True, source_error=source)
            error = error or source
        if error:
            report.update(infrastructure_error=error, blocked_phase=name)
        if row['failed']:
            # The reason, kept small enough for the `phases` artifact, so triage needs no full reports.
            try:
                lines = Path(row['log']).read_text(errors='replace').splitlines()
                row['log_tail'] = [line for line in lines if line.strip()][-LOG_TAIL:]
            except OSError:
                pass
        report['phases'].append(row)
        report['phases'].sort(key=lambda r: order[r['phase']])
        write_report(reports / 'loop.json', report)
        if row['failed'] and name in BARRIER_PHASES or report.get('infrastructure_error'):
            report['blocked_phase'] = name
            return True
        return bool(row['failed'] and args.lane != 'integrate')

    def worker(name, command):
        try:
            row = run_phase(name, command)
        except Exception as error:  # a dead worker would leave the scheduler waiting forever
            row = {'phase': name, 'command': command, 'seconds': 0, 'returncode': None, 'failed': True,
                   'log': str(reports / (name + '.log')), 'outcome_error': f'dev.py: {type(error).__name__}: {error}'}
        with lock:
            running.discard(name)
            finished.add(name)
            if record(row):
                pending.clear()
            lock.notify_all()

    pending = schedule(commands, dependencies)
    # A frozen source is checked between phases, so a change is attributed to exactly one.
    concurrency = 1 if args.frozen_source else args.jobs
    threads = []
    try:
        with lock:
            while pending or running:
                ready = None
                for name, command in pending:
                    exclusive = name in BARRIER_PHASES or serial_phase(name)
                    if dependencies[name] <= finished and not (running and exclusive) \
                            and not any(other in BARRIER_PHASES or serial_phase(other) for other in running) \
                            and len(running) < concurrency:
                        ready = (name, command)
                        break
                    if exclusive:
                        break  # Never let later phases overtake a barrier or serial phase.
                if ready is None:
                    lock.wait()
                    continue
                name, command = ready
                pending.remove(ready)
                error = frozen_error()
                if error:
                    report.update(infrastructure_error=error, blocked_phase=name)
                    pending.clear()
                    continue
                if name.endswith('-unit') or name == 'cargo-build':
                    try:
                        require_build_space(ROOT)
                    except (RuntimeError, ValueError) as error:
                        report.update(infrastructure_error=str(error), blocked_phase=name)
                        pending.clear()
                        continue
                if 'benchmark' in name and any(row['failed'] for row in report['phases']):
                    report['benchmarks_skipped'] = 'Correctness failed; preserve the failure before measuring'
                    pending.clear()
                    continue
                running.add(name)
                thread = threading.Thread(target=worker, args=(name, command), daemon=True)
                threads.append(thread)
                thread.start()
        for thread in threads:
            thread.join()
        failed = report.get('infrastructure_error') or any(row['failed'] for row in report['phases'])
        report['status'] = ('partial_failed' if failed else 'partial_passed') if args.phase or args.skip_phase else ('failed' if failed else 'passed')
        report['full_plan_passed'] = not args.phase and not failed and len(report['phases']) == len(planned)
    finally:
        if report['status'] == 'running':
            report['status'] = 'interrupted'
        executed = {row['phase'] for row in report['phases']}
        selected = {name for name, _ in commands}
        report['not_run'] = [{'phase': name, 'status': 'not_run',
                              'reason': 'explicit_phase_selection' if name not in selected else 'run_stopped_before_phase'}
                             for name, _ in planned if name not in executed]
        report['seconds'] = round(time.monotonic() - started, 4)
        write_report(reports / 'loop.json', report)
        print(f'Loop: {reports / "loop.json"}', flush=True)
        failures = [row for row in report['phases'] if row['failed']]
        if failures:
            lines = [f'{len(failures)} of {len(report["phases"])} phases failed:'] + [
                f'- {row["phase"]} (exit {row.get("returncode")}): {row.get("log")}' for row in failures]
            print(*lines, sep='\n', flush=True)
            if os.environ.get('GITHUB_STEP_SUMMARY'):
                with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as summary:
                    summary.write('### Failed phases\n\n' + '\n'.join(lines) + '\n\n')
    return int(report['status'] not in {'passed', 'partial_passed'})


if __name__ == '__main__':
    sys.exit(main())
