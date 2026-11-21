#!/usr/bin/env python3
"""Dependency-free live protocol framing and local SOCKS5 behavior witnesses."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import socket
import struct
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, run_isolated, server, write_report


def read(sock, count):
    output = b''
    while len(output) < count:
        chunk = sock.recv(count - len(output))
        if not chunk:
            raise AssertionError(f'EOF after {len(output)}/{count} bytes')
        output += chunk
    return output


def mqtt(payload):
    value = len(payload)
    header = b'\x30'
    while True:
        byte = value % 128
        value //= 128
        header += bytes([byte | (128 if value else 0)])
        if not value:
            return header + payload


def framing(engine, protocol, mode):
    cases = []
    with server(engine, Path(__file__).with_name('server.php'), {'TEST_PROTOCOL': protocol, 'TEST_SERVER_MODE': mode}) as (port, _):
        if protocol == 'mqtt':
            wires = [(f'payload-{size}', mqtt(bytes(i % 256 for i in range(size)))) for size in [0, 1, 127, 128, 16383, 16384, 1048576, 2097152]]
            rejected = [('overlong-length', b'\x30\x80\x80\x80\x80')]
        else:
            wires = [('get', b'*1\r\n$3\r\nGET\r\n'), ('empty', b'*1\r\n$0\r\n\r\n'),
                     ('binary', b'*2\r\n$3\r\nSET\r\n$5\r\na\x00b\r\n\r\n'),
                     ('signed-count', b'*+1\r\n$3\r\nGET\r\n')]
            for size in [1048576, 2097152]:
                payload = b'x' * size
                wires += [('payload-' + str(size), b'*1\r\n$' + str(size).encode() + b'\r\n' + payload + b'\r\n')]
            rejected = [('malformed-number', b'*oops\r\n'), ('over-limit', b'*1\r\n$5000000\r\n')]
        for name, wire in wires:
            with socket.create_connection(('127.0.0.1', port), timeout=3) as client:
                client.settimeout(3)
                client.sendall(wire)
                expected = struct.pack('!I', len(wire)) + hashlib.sha256(wire).digest()
                assert read(client, 36) == expected, name
            cases.append({'case': name, 'bytes': len(wire), 'request_sha256': hashlib.sha256(wire).hexdigest(), 'passed': True})
        # Split writes are a transport witness; native prefix checks are the
        # deterministic oracle because TCP may coalesce adjacent writes.
        wire = wires[1][1]
        cuts = [0, 1, len(wire)] if protocol == 'mqtt' else [0, 2, len(wire)]
        with socket.create_connection(('127.0.0.1', port), timeout=3) as client:
            client.settimeout(3)
            for start, end in zip(cuts, cuts[1:]):
                client.sendall(wire[start:end])
                if start == 0:
                    time.sleep(.01)
            assert read(client, 36) == struct.pack('!I', len(wire)) + hashlib.sha256(wire).digest()
        cases.append({'case': 'split-writes', 'passed': True})
        for name, wire in rejected:
            with socket.create_connection(('127.0.0.1', port), timeout=3) as client:
                client.settimeout(3)
                client.sendall(wire)
                try:
                    assert client.recv(1) == b'', name
                    termination = 'eof'
                except ConnectionResetError:
                    termination = 'reset'
            # A fresh valid request verifies that rejection did not kill worker.
            with socket.create_connection(('127.0.0.1', port), timeout=3) as client:
                client.settimeout(3)
                healthy = wires[0][1]
                client.sendall(healthy)
                assert read(client, 36) == struct.pack('!I', len(healthy)) + hashlib.sha256(healthy).digest()
            cases.append({'case': name + '-close-and-health', 'termination': termination, 'passed': True})
    return cases


def proxy(engine, mode, username, password, failure):
    captured = {}
    errors = []
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        listener.listen(1)
        listener.settimeout(5)
        port = listener.getsockname()[1]
        def serve():
            try:
                with listener.accept()[0] as peer:
                    peer.settimeout(3)
                    method = 2 if username and password else 0
                    negotiate = read(peer, 3)
                    assert negotiate == bytes([5, 1, method]), negotiate.hex()
                    captured['negotiate'] = negotiate.hex()
                    peer.sendall(bytes([4 if failure == 'version' else 5, 2 if failure == 'method' else method]))
                    if failure in ['version', 'method']:
                        return
                    if method == 2:
                        auth = read(peer, len(username.encode()) + len(password.encode()) + 3)
                        assert auth == b'\x01' + bytes([len(username)]) + username.encode() + bytes([len(password)]) + password.encode()
                        captured['auth_sha256'] = hashlib.sha256(auth).hexdigest()
                        peer.sendall(b'\x01\x01' if failure == 'auth' else b'\x01\x00')
                        if failure == 'auth':
                            return
                    connect = read(peer, 5)
                    assert connect == b'\x05\x01\x00\x03\x10', connect.hex()
                    destination = read(peer, 18)
                    assert destination == b'destination.test\x01\xbb', destination
                    captured['connect'] = (connect + destination).hex()
                    peer.sendall(bytes([5, 5 if failure == 'connect' else 0, 0, 1, 127, 0, 0, 1, 0, 1]))
                    if failure == 'connect':
                        return
                    assert read(peer, 4) == b'PING'
                    peer.sendall(b'PONG')
            except BaseException as error:
                errors.append(f'{type(error).__name__}: {error}')
        worker = threading.Thread(target=serve, daemon=True)
        worker.start()
        process = run_isolated(engine.command(Path(__file__).with_name('socks.php'), mode, port, username, password), timeout=8)
        worker.join(5)
        assert not worker.is_alive(), 'Proxy fixture did not finish'
        assert not errors, errors
        assert process.returncode == 0, process.stderr + process.stdout
        lines = process.stdout.strip().splitlines()
        outcome = json.loads(lines[-1])
        assert outcome['connected'] == (failure is None), outcome
        assert outcome['received'] == (None if failure else 'PONG'), outcome
        return {'case': mode + '-' + (failure or 'success') + ('-auth-' + str(len(username)) if username and password else '-noauth')
                + ('-partial-user' if username and not password else '-partial-password' if password and not username else ''),
                'passed': True, 'captured': captured, 'outcome': outcome,
                'stderr': process.stderr, 'stdout': process.stdout}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', default=str(ROOT / 'target/reports/protocols-network.json'))
    args = parser.parse_args()
    report = {'created_utc': datetime.now(timezone.utc).isoformat(),
              'scope': 'Local TCP MQTT/RESP framing in base/process modes (2 workers, pinned native64KiB IPC default, packets over2MiB), raw-byte hashes, worker health after rejection; real PHP SOCKS5 sync/coroutine clients against scripted local proxy. Supplements original tests; no external services or Composer/client dependencies.',
              'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        row = {'engine': engine.name, 'cases': [], 'passed': False}
        try:
            for mode in ['base', 'process']:
                for protocol in ['mqtt', 'redis']:
                    row['cases'] += [{'protocol': protocol, 'server_mode': mode, **case} for case in framing(engine, protocol, mode)]
            for mode in ['sync', 'coroutine']:
                for username, password, failure in [('', '', None), ('user', 'pass', None), ('u' * 250, 'p' * 250, None), ('user', '', None), ('', 'pass', None),
                                                     ('', '', 'version'), ('', '', 'method'), ('user', 'pass', 'auth'), ('', '', 'connect')]:
                    row['cases'].append({'protocol': 'socks5', **proxy(engine, mode, username, password, failure)})
            row['passed'] = all(case['passed'] for case in row['cases'])
        except Exception as error:
            row['error'] = f'{type(error).__name__}: {error}'
        report['results'].append(row)
        write_report(args.output, report)
        print(engine.name, 'passed' if row['passed'] else row.get('error'), flush=True)
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
