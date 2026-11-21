"""Additional corpus checks. CLI checks accept (engine, entry); servers accept port."""
import concurrent.futures
import json
import socket
import struct
import subprocess

from tools.harness import request, run_isolated


def expect(actual, expected):
    if actual != expected:
        raise AssertionError(f'expected {expected!r}, got {actual!r}')


def read_exact(connection, length):
    data = bytearray()
    while len(data) < length:
        chunk = connection.recv(length - len(data))
        if not chunk:
            raise AssertionError('unexpected TCP EOF')
        data.extend(chunk)
    return bytes(data)


def frame(payload):
    return struct.pack('!I', len(payload)) + payload


def receive_frame(connection):
    length = struct.unpack('!I', read_exact(connection, 4))[0]
    if length > 131072:
        raise AssertionError('oversized TCP reply')
    return read_exact(connection, length)


def tcp_framing(port):
    binary = bytes(range(256)) * 256
    with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
        packet = frame(binary)
        # Separate writes split both header and body; the callback must see a frame.
        for part in (packet[:2], packet[2:17], packet[17:]):
            connection.sendall(part)
        expect(receive_frame(connection), binary)
        messages = [b'first', b'\0second\xff', b'last']
        connection.sendall(b''.join(map(frame, messages)))
        for payload in messages:
            expect(receive_frame(connection), payload)

    def client(index):
        payload = f'client-{index}'.encode()
        with socket.create_connection(('127.0.0.1', port), timeout=5) as connection:
            connection.sendall(frame(payload))
            expect(receive_frame(connection), payload)

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(client, range(12)))


def cli_json(engine, entry):
    result = run_isolated(engine.command(entry), timeout=10)
    if result.returncode != 0:
        raise AssertionError(f'PHP exited {result.returncode}: {result.stdout}{result.stderr}')
    return json.loads(result.stdout)


def timers(engine, entry):
    expect(cli_json(engine, entry), {'ticks': 3, 'events': ['done'], 'existsBefore': True,
                                    'cleared': True, 'existsAfter': False})


def shared_table(engine, entry):
    expect(cli_json(engine, entry), {'job': {'count': 5, 'label': 'queued'},
                                    'result': {'count': 42, 'label': 'complete'},
                                    'beforeDelete': 2, 'deleted': True, 'missing': False,
                                    'afterDelete': 1})


def task_workers(port):
    for payload, expected in [([1, 2, 3], {'sum': 6, 'count': 3, 'task_worker': True}),
                              ([], {'sum': 0, 'count': 0, 'task_worker': True}),
                              ([-7, 2, 10], {'sum': 5, 'count': 3, 'task_worker': True})]:
        status, _, body = request(port, '/', 'POST', json.dumps(payload))
        expect(status, 200)
        expect(json.loads(body), expected)
    status, _, body = request(port, '/', 'POST', '[1,"invalid"]')
    expect(status, 422)
    expect(json.loads(body), {'error': 'expected integer array'})


TESTS = {'tcp_framing': tcp_framing, 'timers': timers,
         'shared_table': shared_table, 'task_workers': task_workers}
