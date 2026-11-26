"""Independent response-barrier diagnostic; never substitutes for original PHPT."""
from pathlib import Path
import socket
import struct
import time

import pytest

from corpus.run import WebSocket, expect
from tools.harness import request, server


def frame(payload, opcode=1, final=True):
    mask = b'\x00\x01\x80\xff'
    length = len(payload)
    header = bytes([(0x80 if final else 0) | opcode])
    header += (bytes([0x80 | length]) if length < 126 else
               b'\xfe' + struct.pack('!H', length) if length <= 65535 else
               b'\xff' + struct.pack('!Q', length))
    return header + mask + bytes(value ^ mask[i % 4] for i, value in enumerate(payload))


def transport_matrix(port):
    client = WebSocket(port)
    cases = []
    try:
        client.socket.sendall(frame(b'first') + frame(b'second'))
        expect(client.receive(), b'first')
        expect(client.receive(), b'second')
        cases.append('coalesced-text')
        for length in [125, 126, 65535, 65536]:
            payload = b'x' * length
            wire = frame(payload)
            cuts = sorted(set([0, 1, 2, 3, 4, 7, 11, len(wire)] + list(range(4096, len(wire), 4096))))
            for start, end in zip(cuts, cuts[1:]):
                client.socket.sendall(wire[start:end])
            expect(client.receive(), payload)
            cases.append('split-writes-' + str(length))
        client.socket.sendall(frame(b'fragment-', final=False) + frame(b'P', opcode=9) +
                              frame(b'complete', opcode=0))
        expect(client.read(2), b'\x8a\x01')
        expect(client.read(1), b'P')
        expect(client.receive(), b'fragment-complete')
        cases.append('fragment-ping-continuation')
        client.send(b'\x03\xe8', opcode=8)
        expect(client.read(2), b'\x88\x02')
        expect(client.read(2), b'\x03\xe8')
        expect(client.socket.recv(1), b'')
        cases.append('close-eof')
    finally:
        client.close()
    return cases


def test_close(engine):
    """Seven raw transport cases plus the response-barrier diagnostic: two messages
    precede reading the close and text replies; the client stays alive until replies.
    The original send_close_frame PHPT is unchanged (and flaky, #111)."""
    with server(engine, Path(__file__).with_name('close_probe.php')) as (port, _):
        transport_matrix(port)
        client = WebSocket(port)
        try:
            client.send(b'aaa')
            client.send(b'lalalala')
            expect(client.read(2), b'\x88\x0c')
            expect(client.read(12), b'\x03\xe8diagnostic')
            expect(client.receive(), b'lalalala')
            status, _, body = request(port)
            expect(status, 200)
            expect(body, b'healthy')
        finally:
            client.close()


UPGRADE = ('GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n'
           'Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nX-Token: {}\r\n\r\n')


def early(port, token):
    """Sends an upgrade, then a frame while `onHandshake` yields; returns the socket and
    what it read up to the end of the response head."""
    client = socket.create_connection(('127.0.0.1', port), timeout=2)
    client.sendall(UPGRADE.format(token).encode())
    time.sleep(.05)
    client.sendall(frame(b'hi'))
    head = b''
    while not head.endswith(b'\r\n\r\n') and (byte := client.recv(1)):
        head += byte
    return client, head


@pytest.mark.parametrize('mode', ['base', 'process'])
def test_frame_during_handshake(engine, mode):
    """6.3's ordering (7bd6134c4, its early_frame_before_handshake.phpt): a frame sent while
    a custom onHandshake yields is read as HTTP, so onMessage never sees it before the 101.
    A rejected handshake's frame is never a message. An accepted one's, shorter than a
    request line, waits as an incomplete request, and after the open it is the first frame
    the next read parses, as in 6.3."""
    if engine.name == 'upstream':
        pytest.skip('the v6.2.6 pin dispatches it during the handshake')
    with server(engine, Path(__file__).with_name('close_probe.php'), {'TEST_HANDSHAKE': mode}) as (port, _):
        client, head = early(port, 'bad')
        with client:
            assert head.startswith(b'HTTP/1.1 403'), head
            client.recv(4096)
            expect(client.recv(1), b'')
        expect(request(port, '/messages')[2], b'0')
        client, head = early(port, 'good')
        with client:
            assert head.startswith(b'HTTP/1.1 101'), head
            time.sleep(.1)
            expect(request(port, '/messages')[2], b'0')
            client.sendall(frame(b'ok'))
            expect(client.recv(4), b'\x81\x02hi')
            expect(client.recv(4), b'\x81\x02ok')
        expect(request(port, '/messages')[2], b'2')
