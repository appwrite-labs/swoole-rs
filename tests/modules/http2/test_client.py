"""Swoole\\Coroutine\\Http2\\Client against a server that writes raw frames."""
from pathlib import Path
import os
import socket
import struct
import subprocess
import threading

PREFACE = b'PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n'


def frame(kind, flags, stream, payload=b''):
    return struct.pack('!I', len(payload))[1:] + bytes([kind, flags]) + struct.pack('!I', stream) + payload


def read(conn, size):
    data = b''
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError(f'closed after {data!r}')
        data += chunk
    return data


def read_frame(conn):
    head = read(conn, 9)
    return head[3], head[4], read(conn, int.from_bytes(head[:3], 'big'))


def test_ping_ack_echoes_the_payload(engine):
    """RFC 9113 6.7: a PING ACK carries the PING's own eight payload bytes."""
    acks = []
    with socket.create_server(('127.0.0.1', 0)) as listener:
        listener.settimeout(10)

        def serve():
            conn, _ = listener.accept()
            with conn:
                conn.settimeout(10)
                assert read(conn, len(PREFACE)) == PREFACE
                conn.sendall(frame(4, 0, 0) + frame(6, 0, 0, b'pingpong'))
                while not acks:
                    kind, flags, payload = read_frame(conn)
                    if kind == 6 and flags & 1:
                        acks.append(payload)
                conn.sendall(frame(7, 0, 0, struct.pack('!II', 0, 0)))

        thread = threading.Thread(target=serve)
        thread.start()
        php = subprocess.run(engine.command(Path(__file__).with_name('ping_client.php')),
                             env={**os.environ, 'TEST_PORT': str(listener.getsockname()[1])},
                             capture_output=True, text=True, timeout=30)
        thread.join(10)
    assert php.returncode == 0, php.stdout + php.stderr
    assert acks == [b'pingpong'], php.stdout + php.stderr


def test_coroutines_share_a_client(engine):
    """One coroutine parked in recv() while another calls the same client's methods."""
    with socket.create_server(('127.0.0.1', 0)) as listener:
        listener.settimeout(10)

        def serve():
            conn, _ = listener.accept()
            with conn:
                conn.settimeout(10)
                assert read(conn, len(PREFACE)) == PREFACE
                conn.sendall(frame(4, 0, 0))
                # Both requests' HEADERS, the second's body and the PING come while
                # the first coroutine waits in recv(); only then do the responses go.
                seen = set()
                while not {(1, 1), (1, 3), (0, 3), (6, 0)} <= seen:
                    head = read(conn, 9)
                    read(conn, int.from_bytes(head[:3], 'big'))
                    kind, stream = head[3], int.from_bytes(head[5:], 'big') & 0x7fffffff
                    seen.add((kind, stream))
                ok = b'\x88'  # HPACK: `:status: 200`, static index 8.
                conn.sendall(frame(6, 1, 0, bytes(8))
                             + frame(1, 4, 1, ok) + frame(0, 1, 1, b'one')
                             + frame(1, 5, 3, ok))
                while conn.recv(4096):
                    pass

        thread = threading.Thread(target=serve)
        thread.start()
        php = subprocess.run(engine.command(Path(__file__).with_name('concurrent_client.php')),
                             env={**os.environ, 'TEST_PORT': str(listener.getsockname()[1])},
                             capture_output=True, text=True, timeout=30)
        thread.join(10)
    assert php.returncode == 0, php.stdout + php.stderr
    assert php.stdout.splitlines() == [
        'send 1', 'active 1', 'exists true', 'body, current 5', 'send 3', 'current 5', 'ping true',
        'recv 1 200 one', 'recv 3 200 ', 'active 0', 'close true',
    ], php.stdout + php.stderr
