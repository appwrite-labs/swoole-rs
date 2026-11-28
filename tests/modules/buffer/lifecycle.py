#!/usr/bin/env python3
"""Backpressure lifecycle gate with bounded, observable fixture resource checks."""
import argparse
from datetime import datetime, timezone
import http.client
import json
from pathlib import Path
import socket
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, request, server, write_report

PAYLOAD = bytes(range(256)) * 16384


def require(condition, detail):
    if not condition:
        raise AssertionError(detail)


def stats(port):
    status, _, body = request(port, '/stats')
    require(status == 200, f'health status {status}')
    return json.loads(body)


def closed(port, ids):
    deadline = time.monotonic() + 3
    while True:
        state = stats(port)
        if all(state['events'].get(name, {}).get('closes') == 1 for name in ids):
            return state
        require(time.monotonic() < deadline, f'close ledger mismatch: {state}')
        time.sleep(.02)


def start_request(port, path, name):
    connection = socket.create_connection(('127.0.0.1', port), timeout=5)
    connection.sendall(f'GET {path}?id={name} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n'.encode())
    return connection


def response(sock):
    reply = http.client.HTTPResponse(sock)
    reply.begin()
    require(reply.status == 200, f'response status {reply.status}')
    return reply


def run_lifecycle(engine, entry=None):
    """Return every exercised subcase, including failures; lifecycle() is the strict corpus adapter."""
    entry = Path(entry) if entry else Path(__file__).with_suffix('.php')
    evidence = {'cases': [], 'scope': '32-cycle bounded fixture PHP heap/FD check; not native heap leak proof'}
    with server(engine, entry) as (port, process):
        baseline = stats(port)
        names = []
        for name, path, cancel, half in [('slow-reader', '/stream', False, False),
                                          ('queued-stream-disconnect', '/stream', True, False),
                                          ('queued-sendfile-disconnect', '/file', True, False),
                                          ('half-close', '/stream', False, True)]:
            sock = start_request(port, path, name)
            names.append(name)
            case = {'case': name, 'passed': False, 'bytes_read': 0}
            body = bytearray()
            try:
                if half:
                    sock.shutdown(socket.SHUT_WR)
                time.sleep(.1)
                reply = response(sock)
                expected = PAYLOAD[13:-14] if path == '/file' else PAYLOAD
                case['expected_bytes'] = len(expected)
                if cancel:
                    body.extend(reply.read(1024))
                    require(body == expected[:1024], f'{name}: prefix mismatch')
                    case['bytes_read'] = len(body)
                    reply.close()
                else:
                    while chunk := reply.read(16384):
                        require(chunk == expected[len(body):len(body) + len(chunk)], f'{name}: chunk mismatch')
                        body.extend(chunk)
                        case['bytes_read'] = len(body)
                        if name == 'slow-reader':
                            time.sleep(.001)
                    require(body == expected, f'{name}: body mismatch ({len(body)} bytes)')
                    require(sock.recv(1) == b'', f'{name}: missing EOF')
                    reply.close()
                case['passed'] = True
            except Exception as error:
                if isinstance(error, http.client.IncompleteRead):
                    body.extend(error.partial)
                    case['bytes_read'] = len(body)
                    case['received_prefix_valid'] = body == expected[:len(body)]
                case['error'] = f'{type(error).__name__}: {error}'
            finally:
                sock.close()
            state = closed(port, names)
            if path == '/file':
                require(state['events'][name].get('sendfile_accepted') is True, 'sendfile task was not accepted')
            else:
                require(state['events'][name].get('queued_bytes', 0) > 0, f'{name}: output was not queued: {state}')
            case['event'] = state['events'][name]
            case['healthy_after'] = True
            evidence['cases'].append(case)
        for index in range(32):
            name = f'churn-{index}'
            sock = start_request(port, '/', name)
            names.append(name)
            try:
                reply = response(sock)
                require(reply.read() == b'ok', f'{name}: body mismatch')
                reply.close()
            finally:
                sock.close()
        state = closed(port, names)
        require(all(state['events'][name]['requests'] == 1 for name in names), 'request ledger duplicated')
        require(state['process_fds'] is not None and baseline['process_fds'] is not None, 'FD observation unavailable')
        require(state['process_fds'] <= baseline['process_fds'] + 4, f'FD growth exceeded4: {baseline} -> {state}')
        require(state['php_heap_bytes'] <= baseline['php_heap_bytes'] + 4 * 1024 * 1024, f'PHP fixture heap grew over4MiB: {state}')
        require(process.poll() is None, 'server exited before shutdown case')
        evidence['cases'].append({'case': '32-connect-disconnect', 'passed': True, 'closures': len(names),
                                  'baseline': {key: baseline[key] for key in ['process_fds', 'php_heap_bytes']},
                                  'after': {key: state[key] for key in ['process_fds', 'php_heap_bytes']}})
    # Shutdown is a distinct disposable server. A pending response remains unread.
    with tempfile.TemporaryDirectory(prefix='swoole-lifecycle-') as data:
        with server(engine, entry, {'TEST_DATA': data}) as (port, process):
            sock = start_request(port, '/shutdown', 'shutdown')
            try:
                time.sleep(.03)
                snapshot = stats(port)
                require(snapshot['events']['shutdown']['queued_bytes'] > 0, 'shutdown response did not queue')
                started = time.monotonic()
                process.wait(timeout=5)
                require(process.returncode == 0, f'shutdown exit {process.returncode}')
                events = [json.loads(line) for line in (Path(data) / 'events.jsonl').read_text().splitlines()]
                require(sum(event['event'] == 'worker-stop' for event in events) == 1, f'worker-stop count: {events}')
                require(sum(event.get('id') == 'shutdown' for event in events) == 1, f'duplicate shutdown close: {events}')
                evidence['cases'].append({'case': 'pending-write-shutdown', 'passed': True, 'elapsed_seconds': time.monotonic() - started,
                                          'queued_bytes': snapshot['events']['shutdown']['queued_bytes'], 'events': events})
            finally:
                sock.close()
    evidence['passed'] = all(case['passed'] for case in evidence['cases'])
    return evidence


def lifecycle(engine, entry=None):
    evidence = run_lifecycle(engine, entry)
    require(evidence['passed'], json.dumps(evidence))
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    parser.add_argument('--output', default=str(ROOT / 'target/reports/buffer-lifecycle.json'))
    args = parser.parse_args()
    entry = Path(__file__).with_suffix('.php')
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'engines': [], 'results': []}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        row = {'engine': engine.name, 'passed': False}
        try:
            row.update(run_lifecycle(engine, entry))
        except Exception as error:
            row['error'] = f'{type(error).__name__}: {error}'
        report['results'].append(row)
        write_report(args.output, report)
        print(json.dumps(row), flush=True)
    signatures = [[(case['case'], case['passed'], case.get('error', '').split(':', 1)[0])
                   for case in row.get('cases', [])] for row in report['results']]
    report['compatibility_outcomes_equal'] = bool(signatures and signatures[0]) and all(value == signatures[0] for value in signatures[1:])
    report['compatibility_note'] = 'Outcome equality is separate from strict correctness; shared half-close truncation remains a failed case.'
    write_report(args.output, report)
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
