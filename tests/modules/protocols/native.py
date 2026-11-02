#!/usr/bin/env python3
"""Real Rust C ABI + native facades versus unchanged pinned protocol bodies."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.build_hybrid import module_patches
from tools.harness import ROOT, run_isolated, write_report
from tools.harness import run_compiler

from tools.build_upstream import COMMIT as PIN
TARGETS = ['src/protocol/mqtt.cc', 'src/protocol/redis.cc', 'src/protocol/socks5.cc', 'ext-src/swoole_redis_server.cc']


def function(source, signature):
    start = source.index(signature)
    begin = source.index('{', start)
    depth = 1
    end = begin + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, help='External patched checkout; default stages the checked module overlay')
    parser.add_argument('--archive', type=Path, default=ROOT / 'target/protocols/release/libswoole_rs_protocols.a')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/protocols-native.json'))
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    sources = {p: subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:{p}'], text=True) for p in TARGETS}
    headers = {p: subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:{p}'], text=True) for p in ['include/swoole_proxy.h', 'include/swoole_redis.h']}
    if args.candidate:
        candidates = {p: (args.candidate / p).read_text() for p in TARGETS}
    else:
        manifest = module_patches(ROOT / 'migration/patches/protocols')
        with tempfile.TemporaryDirectory(prefix='swr-protocols-overlay-') as directory:
            staged = Path(directory)
            subprocess.run(['git', 'init', '-q', directory], check=True)
            for name in {p for patch in manifest for p in patch['upstream_paths']} | set(TARGETS):
                text = sources.get(name) or subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:{name}'], text=True)
                destination = staged / name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(text)
            for patch in manifest:
                path = ROOT / 'migration/patches/protocols' / patch['path']
                subprocess.run(['git', '-C', directory, 'apply', '--check', str(path)], check=True)
                subprocess.run(['git', '-C', directory, 'apply', str(path)], check=True)
            candidates = {p: (staged / p).read_text() for p in TARGETS}
    archive = args.archive.resolve(strict=True)
    local = Path(__file__).resolve().parent
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
              'scope': 'Actual pinned and selected protocol facade bodies. Native String/allocation/socket IO are modeled, protocol bodies unchanged. The exact original PHP request parser loop runs with modeled Zend parameters on 10000 seeded defined wires versus Rust typed events. Full Zend request/reply callers are staged, not executed here; run request.py/reply.py against actual PHP. No live network in this gate.',
              'sanitizer_scope': 'C++ ASan/UBSan; Rust not instrumented' if args.sanitize else 'none'}
    def bodies(selected):
        mqtt, redis, socks = (selected[p] for p in TARGETS[:3])
        socks_header = headers['include/swoole_proxy.h']
        constants = '\n'.join(line for line in socks_header.splitlines() if line.startswith('#define SW_SOCKS5'))
        socks_types = socks_header[socks_header.index('enum swSocks5State'):socks_header.index('namespace swoole {')]
        socks_types += socks_header[socks_header.index('struct Socks5Proxy {'):socks_header.index('}  // namespace swoole')]
        redis_header = headers['include/swoole_redis.h']
        redis_types = redis_header[redis_header.index('enum State {'):redis_header.index('const char *get_number')]
        redis_constants = '#define SW_REDIS_RETURN_NIL "$-1\\r\\n"\n'
        socks_body = socks[socks.index('const char *Socks5Proxy::strerror'):socks.index('}  // namespace swoole')]
        redis_body = redis[redis.index('namespace redis {') + len('namespace redis {'):redis.index('}  // namespace redis')]
        return (constants + '\n' + socks_types + socks_body + '\nnamespace mqtt {\n' +
                function(mqtt, 'ssize_t get_package_length(') + '\n}\nnamespace redis {\n' +
                redis_constants + redis_types + redis_body + '\n}\n')
    with tempfile.TemporaryDirectory(prefix='swr-protocols-native-') as directory:
        tree = Path(directory)
        for name, selected in [('reference', sources), ('candidate', candidates)]:
            value = bodies(selected)
            (tree / f'{name}.inc').write_text(value)
        php = sources['ext-src/swoole_redis_server.cc']
        php_loop = php[php.index('    const char *p = Z_STRVAL(zdata);'):php.index('\n    if (command_len >=')]
        (tree / 'php-request-original.inc').write_text(php_loop)
        binary = tree / 'native'
        command = ['c++', '-std=c++17', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter', '-Wno-unused-function',
                   '-I' + str(tree), '-I' + str(local), '-I' + str(ROOT / 'native/protocols'),
                   str(local / 'native.cc'), str(archive), '-lc', '-lm']
        if sys.platform.startswith('linux'):
            command += ['-ldl', '-lpthread', '-lrt', '-lutil']
        if args.sanitize:
            command += ['-fsanitize=address,undefined', '-fno-sanitize-recover=all']
        report['compile'] = run_compiler(command + ['-o', str(binary)])
        if report['compile']['returncode']:
            write_report(args.output, report)
            print(report['compile']['stderr'], file=sys.stderr)
            return 1
        result = run_isolated([str(binary)], timeout=60)
        report['run'] = {'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
        report['passed'] = result.returncode == 0
        if not result.returncode:
            report['outcome'] = json.loads(result.stdout)
        write_report(args.output, report)
        print(result.stdout if not result.returncode else result.stderr, end='')
        return int(result.returncode != 0)


if __name__ == '__main__':
    sys.exit(main())
