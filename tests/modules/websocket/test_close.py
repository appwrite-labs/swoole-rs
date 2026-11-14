"""Independent response-barrier diagnostic; never substitutes for original PHPT."""
from pathlib import Path
import struct

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
