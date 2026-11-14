#!/usr/bin/env python3
"""Require intact responses once a graceful close is queued, on real PHP sockets."""
import argparse
from contextlib import closing
import http.client
import json
from pathlib import Path
import socket
import struct
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, request, server, write_report

PAYLOAD = bytes(range(256)) * 16384


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def ledger(port, name):
    status, _, body = request(port, '/stats')
    require(status == 200, f'health status {status}')
    return json.loads(body).get(name, {})


def wait_for(port, name, predicate):
    deadline = time.monotonic() + 4
    while True:
        state = ledger(port, name)
        if predicate(state):
            return state
        require(time.monotonic() < deadline, f'ledger timeout: {name}: {state}')
        time.sleep(.01)


def run(engine, repeat, probe_extra_request):
    rows = []
    for mode in ['base', 'process']:
        with server(engine, Path(__file__).with_name('server.php'), {'TEST_MODE': mode}) as (port, proc):
            for cycle in range(repeat):
                for path in ['/stream', '/file', '/end']:
                    for action in ['fin', 'reset'] + (['extra-request'] if probe_extra_request else []):
                        name = f'{mode}-{cycle}-{path[1:]}-{action}'
                        row = {'case': name, 'passed': False}
                        try:
                            with socket.create_connection(('127.0.0.1', port), timeout=4) as sock, closing(http.client.HTTPResponse(sock)) as reply:
                                sock.sendall(f'GET {path}?id={name} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n'.encode())
                                reply.begin()
                                require(reply.status == 200, f'HTTP {reply.status}')
                                # Same worker and reactor, ordered IPC: this health response follows
                                # the response's close command. Exercise queued-close, not the distinct
                                # process-mode FIN-before-worker-response race.
                                state = wait_for(port, name, lambda state: state.get('finished'))
                                if mode == 'base' and path != '/file':
                                    require(state['queued'] > 0, f'output did not queue: {state}')
                                if action == 'reset':
                                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack('ii', 1, 0))
                                else:
                                    if action == 'extra-request':
                                        sock.sendall(b'GET /end?id=unexpected HTTP/1.1\r\nHost: localhost\r\n\r\n')
                                    sock.shutdown(socket.SHUT_WR)
                                    body = reply.read()
                                    require(body == PAYLOAD, f'incomplete response: {len(body)}/{len(PAYLOAD)} bytes')
                                    require(sock.recv(1) == b'', 'missing EOF')
                                    row['bytes'] = len(body)
                            state = wait_for(port, name, lambda state: state.get('closes') == 1)
                            require(state['requests'] == 1, f'duplicate request: {state}')
                            require(not ledger(port, 'unexpected'), 'request dispatched after queued close')
                            require(proc.poll() is None, 'server died')
                            row.update(passed=True, event=state)
                        except Exception as error:
                            row['error'] = f'{type(error).__name__}: {error}'
                        rows.append(row)
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--probe-extra-request', action='store_true',
                        help='Also run the unresolved unread-input reset/timeout probes; failures remain failures')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/halfclose.json'))
    args = parser.parse_args()
    if args.repeat < 1:
        parser.error('--repeat must be positive')
    report = {'scope': 'queued graceful close only; FIN before a process worker responds remains unsupported',
              'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        try:
            cases = run(engine, args.repeat, args.probe_extra_request)
            row = {'engine': engine.name, 'passed': all(case['passed'] for case in cases), 'cases': cases}
        except Exception as error:
            row = {'engine': engine.name, 'passed': False, 'error': f'{type(error).__name__}: {error}'}
        report['results'].append(row)
        write_report(args.output, report)
        print(json.dumps(row), flush=True)
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
