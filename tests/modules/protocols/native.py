#!/usr/bin/env python3
"""Real Rust C ABI + native facades: the hybrid build's protocol bodies (#796)
versus unchanged pinned ones."""
import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, run_isolated, write_report
from tools.harness import run_compiler

from tools.build_upstream import COMMIT as PIN
TARGETS = ['src/protocol/mqtt.cc', 'src/protocol/socks5.cc']


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
    parser.add_argument('--archive', type=Path, required=True, help='Rust archive of the hybrid build under test')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/protocols-native.json'))
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    sources = {p: subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:{p}'], text=True) for p in TARGETS}
    headers = {p: subprocess.check_output(['git', '-C', str(args.source), 'show', f'{PIN}:{p}'], text=True) for p in ['include/swoole_proxy.h']}
    # Only the pinned protocol bodies are read: the candidate's are Rust, which native.cc calls
    # through native/protocols/swoole_proxy.h.
    archive = args.archive.resolve(strict=True)
    local = Path(__file__).resolve().parent
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'upstream_pin': PIN,
              'scope': 'Actual pinned MQTT and SOCKS5 bodies against the build\'s. Native addresses are modeled, protocol bodies unchanged. RESP is tests/modules/protocols/native.rs. No live network in this gate.',
              'sanitizer_scope': 'C++ ASan/UBSan; Rust not instrumented' if args.sanitize else 'none'}
    def bodies(selected, owned):
        socks_header = headers['include/swoole_proxy.h']
        constants = '\n'.join(line for line in socks_header.splitlines() if line.startswith('#define SW_SOCKS5'))
        socks_types = socks_header[socks_header.index('enum swSocks5State'):socks_header.index('namespace swoole {')]
        # The candidate is production's Socks5Proxy: the hybrid header over Rust's exports
        # under upstream's symbols (crates/protocols-ffi/src/owned_proxy.rs).
        socks_types += ('using Socks5Proxy = ::swoole::Socks5Proxy;\n' if owned else
                        socks_header[socks_header.index('struct Socks5Proxy {'):socks_header.index('}  // namespace swoole')])
        socks = '' if owned else selected[TARGETS[1]]
        socks_body = socks and socks[socks.index('const char *Socks5Proxy::strerror'):socks.index('}  // namespace swoole')]
        # The candidate's mqtt::get_package_length is Rust's export under the upstream symbol (native.cc).
        mqtt = '' if owned else '\nnamespace mqtt {\n' + function(selected[TARGETS[0]], 'ssize_t get_package_length(') + '\n}'
        return constants + '\n' + socks_types + socks_body + mqtt + '\n'
    with tempfile.TemporaryDirectory(prefix='swr-protocols-native-') as directory:
        tree = Path(directory)
        for name, selected in [('reference', sources), ('candidate', {})]:
            value = bodies(selected, name == 'candidate')
            (tree / f'{name}.inc').write_text(value)
        binary = tree / 'native'
        command = ['c++', '-std=c++17', '-O2', '-Wall', '-Wextra', '-Werror', '-Wno-unused-parameter', '-Wno-unused-function',
                   '-I' + str(tree), '-I' + str(local), '-I' + str(ROOT / 'native/protocols'), '-I'+str(ROOT/'native/include'), '-I' + str(ROOT / 'native/http'),
                   str(local / 'native.cc'), str(archive), '-lc', '-lm']
        if sys.platform.startswith('linux'):
            command += ['-ldl', '-lpthread', '-lrt', '-lutil', '-Wl,--gc-sections']
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
