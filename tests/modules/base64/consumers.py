#!/usr/bin/env python3
"""Exercise actual Http Client basic-auth and WebSocket handshake codec callers.

Only loopback/ephemeral listeners, no external service or benchmark. The checked
extension and its build manifest are supplied explicitly; artifacts stay immutable.
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tools.harness import run_isolated


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--extension', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    extension = args.extension.resolve(strict=True)
    manifest = json.loads(args.manifest.read_text())
    assert digest(extension) == manifest['extension_sha256'], 'Extension differs from supplied manifest'
    report = {'passed': False, 'extension': str(extension), 'manifest': str(args.manifest.resolve()), 'commands': []}
    command = [args.php, '-n']
    for module in args.php_extension:
        command.extend(['-d', f'extension={module}'])
    command.extend(['-d', f'extension={extension}', str(Path(__file__).with_suffix('.php'))])
    vectors = [(b'', b''), (b'u', b'p'), (b'username', b'password'),
               (bytes(range(256)), bytes(reversed(range(256)))), (b'u\0ser', b'pass\0word')]
    vectors += [(bytes((i * 37 + n) & 255 for i in range(n)), b'p') for n in (2, 3, 4, 5, 20, 64, 1024, 8192)]
    expected = [b'Basic ' + base64.b64encode(user + b':' + password) for user, password in vectors]
    received = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            received.append(self.headers.get('Authorization', self.headers.get('Proxy-Authorization', '')).encode('ascii'))
            self.send_response(200)
            self.send_header('Content-Length', '0')
            self.end_headers()

        def log_message(self, *_):
            pass

    try:
        # Server ownership stays on the main thread; the bounded client runs in
        # another thread so no daemon/request survives a failed client.
        with http.server.HTTPServer(('127.0.0.1', 0), Handler) as server, tempfile.TemporaryDirectory(prefix='swr-base64-callers-') as temporary:
            server.timeout = 0.1
            proxy_vectors = [(u, p) for u, p in vectors if u and p]
            proxy_expected = [b'Basic ' + base64.b64encode((u.split(b'\0')[0] + b':' + p.split(b'\0')[0])[:255])
                              for u, p in proxy_vectors]
            for mode, inputs_list, expected_headers in [('client', vectors, expected), ('proxy', proxy_vectors, proxy_expected)]:
                received.clear()
                inputs = Path(temporary) / 'inputs.json'
                inputs.write_text(json.dumps([(u.hex(), p.hex()) for u, p in inputs_list]))
                results = []
                def run_client():
                    try:
                        results.append(run_isolated([*command, mode, str(server.server_port), str(inputs)], timeout=30))
                    except Exception as error:
                        results.append(error)
                thread = threading.Thread(target=run_client)
                thread.start()
                while thread.is_alive():
                    server.handle_request()
                thread.join()
                if isinstance(results[0], Exception):
                    raise results[0]
                result = results[0]
                report['commands'].append({'argv': [*command, mode, str(server.server_port), '<inputs.json>'],
                                           'returncode': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr})
                result.check_returncode()
                assert json.loads(result.stdout)['basic_auth_requests'] == len(expected_headers)
                assert received == expected_headers, 'Actual native ' + mode + ' header differs from independent Python Base64'
                report['basic_auth' if mode == 'client' else 'proxy_auth'] = {'passed': True, 'requests': len(received)}

        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        server = subprocess.Popen([*command, 'server', str(port)], stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            assert select.select([server.stdout], [], [], 10)[0], 'WebSocket server readiness timeout'
            ready = server.stdout.readline()
            assert ready == 'READY\n', 'WebSocket server did not start: ' + ready
            keys = [base64.b64encode(bytes((i * 37 + n) & 255 for i in range(16))) for n in range(32)]
            for key in keys:
                with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
                    connection.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ' + key + b'\r\n\r\n')
                    header = b''
                    while b'\r\n\r\n' not in header:
                        chunk = connection.recv(4096)
                        assert chunk, 'Handshake ended before headers'
                        header += chunk
                    lines = header.split(b'\r\n')
                    assert lines[0].startswith(b'HTTP/1.1 101'), header
                    fields = {line.split(b':', 1)[0].lower(): line.split(b':', 1)[1].strip() for line in lines[1:] if b':' in line}
                    expected_accept = base64.b64encode(hashlib.sha1(key + b'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest())
                    assert fields[b'sec-websocket-accept'] == expected_accept, header
            report['websocket'] = {'passed': True, 'handshakes': len(keys)}
        finally:
            if server.poll() is None:
                os.killpg(server.pid, signal.SIGTERM)
            try:
                stdout, stderr = server.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(server.pid, signal.SIGKILL)
                stdout, stderr = server.communicate()
            report['server_output'] = {'stdout': stdout, 'stderr': stderr, 'returncode': server.returncode}
        report['passed'] = True
    except Exception as error:
        report['error'] = str(error)
        raise
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({key: report[key] for key in ('passed', 'basic_auth', 'proxy_auth', 'websocket')}))


if __name__ == '__main__':
    main()
