#!/usr/bin/env python3
"""Keep application read pauses intact while real TCP or TLS output drains."""
import argparse
import json
from pathlib import Path
import socket
import ssl
import struct
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, server, write_report
from tests.modules.halfclose.gate import PAYLOAD, require


TLS_CONTEXT = None


def connect(port):
    sock = socket.create_connection(('127.0.0.1', port), timeout=5)
    if TLS_CONTEXT is None:
        return sock
    try:
        return TLS_CONTEXT.wrap_socket(sock, server_hostname='localhost', suppress_ragged_eofs=False)
    except BaseException:
        sock.close()
        raise


def control(port, command='stats'):
    with connect(port) as sock:
        sock.sendall(command.encode() + b'\n')
        with sock.makefile('rb') as reply:
            return json.loads(reply.readline())


def wait_for(port, name, predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while True:
        state = control(port)['events'].get(name, {})
        if predicate(state):
            return state
        require(time.monotonic() < deadline, f'ledger timeout: {name}: {state}')
        time.sleep(.005)


def read_exact(sock, size):
    data = bytearray()
    while len(data) < size:
        chunk = sock.recv(min(65536, size - len(data)))
        require(chunk, f'EOF after {len(data)}/{size} bytes')
        data.extend(chunk)
    return data


def drain(sock):
    require(read_exact(sock, len(PAYLOAD)) == PAYLOAD, 'binary payload mismatch')


def remains_paused(port, name, expected):
    # Successful control callbacks keep proving the worker is live while queued input waits.
    deadline = time.monotonic() + .1
    observations = 0
    while observations < 5 or time.monotonic() < deadline:
        state = control(port)['events'][name]
        require(state['more'] == expected, f'input dispatched without resume: {state}')
        observations += 1
        time.sleep(.005)


def close_immediate(sock, port, name, row):
    data = bytearray()
    deadline = time.monotonic() + 20
    while len(data) < len(PAYLOAD):
        chunk = sock.recv(min(16384, len(PAYLOAD) - len(data)))
        require(chunk, 'early data EOF')
        data.extend(chunk)
        if len(data) >= len(PAYLOAD) - 262144:
            info = control(port, f'queue:{name}')['ok']
            if info['send_queued_bytes'] == 0:
                break
        require(time.monotonic() < deadline, 'immediate-close reader deadline')
    row['received_before_close'] = len(data)
    require(len(data) < len(PAYLOAD), 'no unread tail at empty queue')
    row['queue_before_close'] = control(port, f'queue:{name}')['ok']['send_queued_bytes']
    require(row['queue_before_close'] == 0, 'application output still queued')
    wait_for(port, name, lambda state: state.get('drained', 0) >= 1)
    require(control(port, f'finish-only:{name}')['ok'], 'immediate close rejected')
    data.extend(read_exact(sock, len(PAYLOAD) - len(data)))
    require(data == PAYLOAD, 'immediate-close binary payload mismatch')
    row['bytes'] = len(data)
    require(sock.recv(1) == b'', 'missing immediate-close TLS close_notify')


def run(engine, repeat, coroutines, tls_env, immediate_only=False):
    rows = []
    for mode in ['base', 'process']:
        with server(engine, Path(__file__).with_suffix('.php'),
                    {'TEST_MODE': mode, 'TEST_COROUTINES': str(int(coroutines)), **tls_env}) as (port, proc):
            baseline = control(port)['connections']
            for cycle in range(repeat):
                for scenario in (['immediate-close'] if immediate_only else
                                 ['paused', 'unpaused', 'early-resume', 'repause', 'close-resume', 'reset', 'force']):
                    name = f'{mode}-{cycle}-{scenario}'
                    row = {'case': name, 'passed': False}
                    try:
                        with connect(port) as sock:
                            if immediate_only:
                                sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 32768)
                                row['receive_buffer'] = sock.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF)
                            if TLS_CONTEXT is not None:
                                row['tls'] = {'version': sock.version(), 'cipher': sock.cipher()}
                            command = 'start' if scenario == 'unpaused' else 'start-paused'
                            sock.sendall(f'{command}:{name}\n'.encode())
                            state = wait_for(port, name, lambda state: state.get('full', 0) >= 1)
                            require(state['sent'], f'initial send rejected: {state}')
                            require(state['paused'] == (scenario != 'unpaused'), f'pause rejected: {state}')
                            if scenario == 'immediate-close':
                                close_immediate(sock, port, name, row)
                            elif scenario == 'reset':
                                sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack('ii', 1, 0))
                            elif scenario == 'force':
                                require(control(port, f'force:{name}')['ok'], 'forced close rejected')
                            elif scenario == 'close-resume':
                                require(control(port, f'finish:{name}')['ok'], 'queued close/resume rejected')
                                if TLS_CONTEXT is None:
                                    sock.shutdown(socket.SHUT_WR)
                                drain(sock)
                                row['bytes'] = len(PAYLOAD)
                                require(sock.recv(1) == b'', 'missing queued-close EOF')
                            else:
                                sock.sendall(f'more:{name}\n'.encode())
                                if scenario == 'early-resume':
                                    require(control(port, f'resume:{name}')['ok'], 'early resume rejected')
                                    require(control(port, f'resume:{name}')['ok'], 'repeated resume rejected')
                                    wait_for(port, name, lambda state: state.get('more') == 1, timeout=1)
                                drain(sock)
                                wait_for(port, name, lambda state: state.get('drained', 0) >= 1)
                                if scenario in ['paused', 'repause']:
                                    remains_paused(port, name, 0)
                                    require(control(port, f'resume:{name}')['ok'], 'resume rejected')
                                require(read_exact(sock, 5) == b'more\n', 'missing resumed input receipt')
                                wait_for(port, name, lambda state: state.get('more') == 1)
                                if scenario == 'repause':
                                    require(control(port, f'pause:{name}')['ok'], 'second pause rejected')
                                    require(control(port, f'send:{name}')['ok'], 'send while paused rejected')
                                    sock.sendall(f'more:{name}\n'.encode())
                                    drain(sock)
                                    wait_for(port, name, lambda state: state.get('drained', 0) >= 2)
                                    remains_paused(port, name, 1)
                                    require(control(port, f'resume:{name}')['ok'], 'second resume rejected')
                                    require(read_exact(sock, 5) == b'more\n', 'missing second resumed receipt')
                                    wait_for(port, name, lambda state: state.get('more') == 2)
                                row['bytes'] = len(PAYLOAD) * (2 if scenario == 'repause' else 1)
                        state = wait_for(port, name, lambda state: state.get('closes') == 1)
                        require(proc.poll() is None, 'server died')
                        row.update(passed=True, event=state)
                    except Exception as error:
                        row['error'] = f'{type(error).__name__}: {error}'
                        # A failed paused case still needs an explicit owner to release its socket.
                        try:
                            control(port, f'force:{name}')
                        except Exception as cleanup_error:
                            row['cleanup_error'] = str(cleanup_error)
                    rows.append(row)
            snapshot = control(port)
            require(snapshot['connections'] <= baseline + 1, f'connection growth: {baseline} -> {snapshot}')
            for row in rows:
                if not row['case'].startswith(mode + '-'):
                    continue
                state = snapshot['events'].get(row['case'], {})
                if state.get('starts') != 1 or state.get('closes') != 1:
                    row.update(passed=False, final_lifecycle_error=f'final ledger mismatch: {state}')
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--coroutines', action='store_true')
    parser.add_argument('--tls-cert', type=Path)
    parser.add_argument('--tls-key', type=Path)
    parser.add_argument('--tls-version', choices=['1.2', '1.3'])
    parser.add_argument('--immediate-close-only', action='store_true')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/paused-reads.json'))
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('--repeat must be positive')
    if args.immediate_close_only and not args.tls_cert:
        parser.error('--immediate-close-only requires TLS cert/key')
    if args.tls_version and not args.tls_cert:
        parser.error('--tls-version requires --tls-cert and --tls-key')
    if bool(args.tls_cert) != bool(args.tls_key):
        parser.error('--tls-cert and --tls-key are required together')
    global TLS_CONTEXT
    tls_env = {}
    if args.tls_cert:
        # Pinned local test certificate; no external endpoint or trust-store changes.
        TLS_CONTEXT = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        TLS_CONTEXT.check_hostname = False
        TLS_CONTEXT.verify_mode = ssl.CERT_NONE
        TLS_CONTEXT.options &= ~getattr(ssl, 'OP_IGNORE_UNEXPECTED_EOF', 0)
        if args.tls_version:
            version = getattr(ssl.TLSVersion, 'TLSv' + args.tls_version.replace('.', '_'))
            try:
                TLS_CONTEXT.minimum_version = TLS_CONTEXT.maximum_version = version
            except ValueError as error:
                parser.error(f'{ssl.OPENSSL_VERSION}: {error}')
        tls_env = {'TEST_TLS_CERT': str(args.tls_cert.resolve()), 'TEST_TLS_KEY': str(args.tls_key.resolve())}
    report = {'scope': 'explicit read pause ownership through output drain; BASE and PROCESS',
              'coroutines': args.coroutines, 'immediate_close_only': args.immediate_close_only, 'transport': 'tls' if tls_env else 'tcp',
              'requested_tls_version': args.tls_version,
              'python': sys.version, 'ssl_options': int(TLS_CONTEXT.options) if tls_env else None,
              'tls_eof_policy': 'require close_notify; no raw TCP half-close' if tls_env else None,
              'python_openssl': ssl.OPENSSL_VERSION if tls_env else None,
              'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        try:
            cases = run(engine, args.repeat, args.coroutines, tls_env, args.immediate_close_only)
            row = {'engine': engine.name, 'passed': all(case['passed'] for case in cases), 'cases': cases}
        except Exception as error:
            row = {'engine': engine.name, 'passed': False, 'error': f'{type(error).__name__}: {error}'}
        report['results'].append(row)
        write_report(args.output, report)
        print(json.dumps(row), flush=True)
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
