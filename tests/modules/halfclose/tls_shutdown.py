#!/usr/bin/env python3
"""Exercise actual Socket TLS shutdown methods with real OpenSSL and explicit error injection."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--cert', type=Path, required=True)
    parser.add_argument('--key', type=Path, required=True)
    parser.add_argument('--openssl-dir', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source = (args.source / 'src/network/socket.cc').read_text()
    methods = source[source.index('bool Socket::ssl_shutdown() {'):source.index('void Socket::ssl_catch_error() const {')]
    driver = Path(__file__).with_suffix('.cc').read_text()
    assert driver.count('// ACTUAL_SOCKET_METHODS') == 1
    report = {'scope': 'Actual extracted method bodies, real memory-BIO and nonblocking socketpair TLS1.2 peers; error adapter replaces only SSL_shutdown/SSL_get_error for five explicit negative-result cases. Native method instrumentation; OpenSSL library uninstrumented. No claim of full native object-layout/reactor coverage.',
              'source': str(args.source.resolve())}
    with tempfile.TemporaryDirectory(prefix='tls-shutdown-') as temp:
        temp = Path(temp)
        (temp / 'test.cc').write_text(driver.replace('// ACTUAL_SOCKET_METHODS', methods))
        flags = ['-I' + str(args.openssl_dir / 'include'), '-L' + str(args.openssl_dir / 'lib')] if args.openssl_dir else []
        command = [os.environ.get('CXX', 'c++'), '-std=c++17', '-fsanitize=address,undefined', '-fno-omit-frame-pointer', *flags, str(temp / 'test.cc'), '-lssl', '-lcrypto', '-o', str(temp / 'test')]
        result = subprocess.run(command, capture_output=True, text=True, timeout=60)
        report['compile'] = {'command': shlex.join(command), 'exit_code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
        if result.returncode == 0:
            result = subprocess.run([str(temp / 'test'), str(args.cert.resolve()), str(args.key.resolve())], capture_output=True, text=True, timeout=20)
            report['run'] = {'exit_code': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}
        report['passed'] = result.returncode == 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))
    return int(not report['passed'])


if __name__ == '__main__':
    raise SystemExit(main())
