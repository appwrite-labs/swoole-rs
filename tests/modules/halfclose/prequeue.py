#!/usr/bin/env python3
"""Require synchronous process workers to finish responses after an early client FIN."""
import argparse
from contextlib import closing
import http.client
import json
from pathlib import Path
import socket
import struct
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, request, server, write_report
from tests.modules.halfclose.gate import PAYLOAD, require, wait_for


def run(engine, repeat, coroutines, workers, keep_alive, max_queued_bytes, unordered):
    cases = []
    connection = 'keep-alive' if keep_alive else 'close'
    with tempfile.TemporaryDirectory(prefix='swoole-prequeue-') as directory:
        with server(engine, Path(__file__).with_suffix('.php'),
                    {'TEST_DATA': directory, 'TEST_COROUTINES': str(int(coroutines)),
                     'TEST_WORKERS': str(workers), 'TEST_MAX_QUEUED_BYTES': str(max_queued_bytes),
                     'TEST_UNORDERED': str(int(unordered))}) as (port, proc):
            for cycle in range(repeat):
                for delay in [0, 25000, 100000]:
                    for route in ['stream', 'file', 'end', 'force']:
                        for action in ['fin', 'reset']:
                            name = f'{cycle}-{delay}-{route}-{action}'
                            row = {'case': name, 'passed': False}
                            try:
                                with socket.create_connection(('127.0.0.1', port), timeout=4) as sock, closing(http.client.HTTPResponse(sock)) as reply:
                                    sock.sendall(f'GET /{route}?id={name}&delay={delay} HTTP/1.1\r\nHost: localhost\r\nConnection: {connection}\r\n\r\n'.encode())
                                    if action == 'fin':
                                        # FIN precedes even the first response header; no ready-response barrier.
                                        sock.shutdown(socket.SHUT_WR)
                                        if route == 'force':
                                            try:
                                                require(sock.recv(1) == b'', 'forced close sent response bytes')
                                            except ConnectionResetError:
                                                pass
                                        else:
                                            reply.begin()
                                            require(reply.status == 200, f'HTTP {reply.status}')
                                            body = reply.read()
                                            require(body == PAYLOAD, f'body length {len(body)}/{len(PAYLOAD)}')
                                            require(sock.recv(1) == b'', 'missing EOF')
                                            row['bytes'] = len(body)
                                    else:
                                        # Ensure reset happens after dispatch rather than losing the request.
                                        deadline = time.monotonic() + 4
                                        while not (Path(directory) / name).exists():
                                            require(time.monotonic() < deadline, 'worker did not enter request')
                                            time.sleep(.001)
                                        sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack('ii', 1, 0))
                                row['passed'] = True
                            except Exception as error:
                                row['error'] = f'{type(error).__name__}: {error}'
                            try:
                                state = wait_for(port, name, lambda state: state.get('finished') and state.get('closes') == 1)
                                require(state['requests'] == 1, f'duplicate request: {state}')
                                if route == 'force':
                                    require(state.get('force_closed') and not state['accepted'], f'forced close was not terminal: {state}')
                                elif action == 'fin':
                                    require(state['accepted'], f'response was rejected: {state}')
                                require(proc.poll() is None, 'server died')
                                row['event'] = state
                            except Exception as error:
                                row.update(passed=False, lifecycle_error=f'{type(error).__name__}: {error}')
                            cases.append(row)
            status, _, body = request(port, '/stats')
            require(status == 200, f'final health status {status}')
            snapshot = json.loads(body)
            for row in cases:
                state = snapshot.get(row['case'], {})
                if state.get('requests') != 1 or state.get('closes') != 1:
                    row.update(passed=False, final_lifecycle_error=f'final ledger mismatch: {state}')
    return cases


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--workers', type=int, choices=[1, 2], default=1)
    parser.add_argument('--keep-alive', action='store_true')
    parser.add_argument('--max-queued-bytes', type=int, default=0)
    parser.add_argument('--probe-unordered', action='store_true', help='Probe unsupported round dispatch; failures remain failures')
    parser.add_argument('--probe-coroutines', action='store_true', help='Probe the separately unresolved suspended-coroutine lifetime; failures remain failures')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/prequeue.json'))
    args = parser.parse_args()
    if args.max_queued_bytes < 0:
        parser.error('--max-queued-bytes must be nonnegative')
    if args.repeat < 1:
        parser.error('--repeat must be positive')
    report = {'settings': {'workers': args.workers, 'keep_alive': args.keep_alive, 'max_queued_bytes': args.max_queued_bytes, 'coroutines': args.probe_coroutines, 'unordered': args.probe_unordered}, 'scope': 'early FIN with synchronous PROCESS responses; coroutine probe is explicit',
              'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        try:
            cases = run(engine, args.repeat, args.probe_coroutines, args.workers, args.keep_alive, args.max_queued_bytes, args.probe_unordered)
            row = {'engine': engine.name, 'passed': all(case['passed'] for case in cases), 'cases': cases}
        except Exception as error:
            row = {'engine': engine.name, 'passed': False, 'error': f'{type(error).__name__}: {error}'}
        report['results'].append(row)
        write_report(args.output, report)
        print(json.dumps(row), flush=True)
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
