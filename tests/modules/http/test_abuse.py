"""Resource abuse sent to a whole server on the wire (#5707), judged against fixed bounds.

Each case is one abusive peer on a fresh server at default limits (abuse.php). It passes when
the server ends up in an expected state, its peak resident memory grows by at most RSS_BOUND
over its idle size, and the connection is answered or closed within the case's time bound.
No engine is the control: a bound comes from the limit that protects the case, and #5702's
rule that well-formed traffic never reaches one.
"""
import functools
from pathlib import Path
import socket
import struct
import sys
import time
import zlib

import pytest

from corpus.run import WebSocket
from tests.modules.http2.test_client import PREFACE, frame
from tools.harness import request, server

pytestmark = pytest.mark.skipif(not sys.platform.startswith('linux'), reason='reads /proc')

MIB = 1 << 20
PACKAGE_MAX = 2 * MIB  # package_max_length's default: the body and decoded WebSocket message limit
HEADER_MAX = 64 * 1024  # SW_HTTP_HEADER_MAX_SIZE
RSS_BOUND = 32 * MIB  # a few times the largest buffer one of these limits allows
BOMB = 256 * MIB  # inflates to 128 times PACKAGE_MAX; deflates to about 256 KiB
PONG = b'\x8a\x00'
PING_ACK = frame(6, 1, 0, b'abuseend')
GET = b'\x82\x84\x86\x41\x09localhost'  # :method GET, :path /, :scheme http (RFC 7541 Appendix A), :authority


def kib(pid, field):
    for line in Path(f'/proc/{pid}/status').read_text().splitlines():
        if line.startswith(field + ':'):
            return int(line.split()[1]) * 1024
    raise AssertionError(f'{field} missing for {pid}')


def ending(conn, limit, done=lambda received: False):
    """Read until the server closes, DONE holds or LIMIT seconds pass: what it sent, and whether it closed."""
    received, deadline = b'', time.monotonic() + limit
    while not done(received) and (left := deadline - time.monotonic()) > 0:
        conn.settimeout(left)
        try:
            chunk = conn.recv(65536)
        except TimeoutError:
            break
        except ConnectionResetError:
            return received, True
        if not chunk:
            return received, True
        received += chunk
    return received, False


@functools.cache
def bomb(wbits):
    """BOMB zero bytes compressed: gzip for wbits 31, a permessage-deflate payload for -15."""
    compressor, zeros = zlib.compressobj(9, zlib.DEFLATED, wbits), bytes(MIB)
    data = b''.join(compressor.compress(zeros) for _ in range(BOMB // MIB))
    return data + compressor.flush() if wbits > 0 else (data + compressor.flush(zlib.Z_SYNC_FLUSH))[:-4]


@pytest.fixture(scope='module', autouse=True)
def bombs():
    """Compressed before any case's clock starts."""
    bomb(31), bomb(-15)


def http_header_count(port):
    """As many distinct headers as fit under the header limit, twenty times on one connection."""
    names = [b'h%x: v' % i for i in range(HEADER_MAX // 10)]
    head = b'\r\n'.join([b'GET / HTTP/1.1', b'Host: localhost', *names, b'', b''])
    assert len(head) < HEADER_MAX
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        for _ in range(20):
            conn.sendall(head)
            received = b''
            while not received.endswith(b'\r\n\r\n%d 0' % (len(names) + 1)):
                if not (chunk := conn.recv(65536)):
                    return 'closed'
                received += chunk
    return received.split(b'\r\n', 1)[0].decode()


def http_slowloris(port):
    """A header line one byte every 250 ms for 8 s, never finishing the request."""
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        conn.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\n')
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                conn.sendall(b'x')
            except OSError:
                return 'closed'
            if ending(conn, .25)[1]:
                return 'closed'
    return 'open'


def http_gzip_body(port):
    """A gzip request body that would inflate to 128 times package_max_length: the body stays as sent."""
    body = bomb(31)
    head = b'POST / HTTP/1.1\r\nHost: localhost\r\nContent-Encoding: gzip\r\nConnection: close\r\nContent-Length: %d\r\n\r\n'
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        conn.sendall(head % len(body) + body)
        received, closed = ending(conn, 5)
    return received.split(b'\r\n', 1)[0].decode() if received.endswith(b' %d' % len(body)) else 'closed' if closed else 'open'


def ws_frame(payload, opcode=1, final=True, rsv1=False):
    """A client frame masked with zeros, so the payload goes on the wire unchanged."""
    size = len(payload)
    length = bytes([0x80 | size]) if size < 126 else b'\xfe' + size.to_bytes(2) if size < 65536 else b'\xff' + size.to_bytes(8)
    return bytes([(0x80 if final else 0) | (0x40 if rsv1 else 0) | opcode]) + length + bytes(4) + payload


def ws_outcome(port, data, deflate=False):
    """DATA, then a ping: a lone pong shows the server dropped DATA and kept the connection."""
    client = WebSocket(port, deflate=deflate)
    try:
        try:
            client.socket.sendall(data + ws_frame(b'', 9))
        except OSError:
            pass  # the server may close before reading everything
        received, closed = ending(client.socket, 10, lambda received: received.endswith(PONG))
    finally:
        client.close()
    if received.startswith(b'\x88'):
        return 'close %d' % int.from_bytes(received[2:4])
    if received.endswith(PONG):
        return 'dropped' if received == PONG else 'delivered'
    return 'closed' if closed else 'open'


def ws_deflate_bomb(port):
    return ws_outcome(port, ws_frame(bomb(-15), rsv1=True), deflate=True)


def ws_tiny_fragments(port):
    """One-byte fragments until the message is a byte over package_max_length."""
    return ws_outcome(port, ws_frame(b'a', final=False) + ws_frame(b'a', 0, False) * (PACKAGE_MAX - 1) + ws_frame(b'a', 0))


def h2_outcome(port, frames, settings=b'', until=PING_ACK):
    """FRAMES after the preface, then a PING: 'ack' once the server answers it, or how it ended the connection."""
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        try:
            conn.sendall(PREFACE + frame(4, 0, 0, settings) + frames + frame(6, 0, 0, PING_ACK[9:]))
        except OSError:
            pass
        received, closed = ending(conn, 10, lambda received: until in received)
    at = 0
    while at + 9 <= len(received):
        if received[at + 3] == 7:
            return 'goaway %d' % int.from_bytes(received[at + 13:at + 17]) + ('' if closed else ', open')
        at += 9 + int.from_bytes(received[at:at + 3])
    return 'ack' if PING_ACK in received else 'closed' if closed else 'open'


def h2_hpack_references(port):
    """A 4000-byte dynamic-table entry referenced 200,000 times (800 MB decoded) in one header block."""
    block = GET + b'\x40\x01a\x7f\xa1\x1e' + b'x' * 4000 + b'\xbe' * 200_000
    chunks = [block[at:at + 16384] for at in range(0, len(block), 16384)]
    return h2_outcome(port, frame(1, 1, 1, chunks[0]) + b''.join(
        frame(9, 4 if chunk is chunks[-1] else 0, 1, chunk) for chunk in chunks[1:]))


def h2_stream_flood(port):
    """50,000 streams opened and never ended."""
    return h2_outcome(port, b''.join(frame(1, 4, stream, GET) for stream in range(1, 100_000, 2)))


def h2_rapid_reset(port):
    """50,000 streams each opened and at once cancelled."""
    return h2_outcome(port, b''.join(frame(1, 4, stream, GET) + frame(3, 0, stream, struct.pack('!I', 8))
                                     for stream in range(1, 100_000, 2)))


def h2_no_window_update(port):
    """SETTINGS_INITIAL_WINDOW_SIZE 0 and never a WINDOW_UPDATE: the response waits until send_timeout."""
    return h2_outcome(port, frame(1, 5, 1, GET), struct.pack('!HI', 4, 0), until=b'\0' * 64)


ENDED = {'close 1009', 'closed', 'dropped'}
CASES = {
    # name: (attack, outcomes that pass, seconds the connection may take)
    'http-slowloris': (http_slowloris, {'closed'}, 8),
    'http-header-count': (http_header_count, {'HTTP/1.1 200 OK'}, 2),
    'http-gzip-body': (http_gzip_body, {'HTTP/1.1 200 OK'}, 2),
    'ws-deflate-bomb': (ws_deflate_bomb, ENDED, 2),
    'ws-tiny-fragments': (ws_tiny_fragments, ENDED, 5),
    'h2-hpack-references': (h2_hpack_references, {'ack'}, 2),
    'h2-stream-flood': (h2_stream_flood, {'ack', 'goaway 11', 'closed'}, 5),
    'h2-rapid-reset': (h2_rapid_reset, {'ack', 'goaway 11', 'closed'}, 5),
    'h2-no-window-update': (h2_no_window_update, {'goaway 0'}, 3),
}
BLOCKED = {
    'http-slowloris': '#5702: nothing ends a request head that never completes; no ticket yet',
    'h2-stream-flood': '#5704: no per-connection budget caps the streams a peer keeps open',
}


@pytest.mark.parametrize('name', [pytest.param(name, marks=pytest.mark.xfail(strict=True, reason=BLOCKED[name]))
                                  if name in BLOCKED else name for name in CASES])
def test_abuse(engine, name):
    attack, passing, seconds = CASES[name]
    with server(engine, Path(__file__).with_name('abuse.php')) as (port, proc):
        assert request(port)[0] == 200
        pid = engine.pid(proc)
        Path(f'/proc/{pid}/clear_refs').write_text('5')  # restart VmHWM from the current RSS
        idle = kib(pid, 'VmRSS')
        start = time.monotonic()
        outcome = attack(port)
        elapsed = time.monotonic() - start
        growth = kib(pid, 'VmHWM') - idle
        print(f'{name} {engine.name}: {outcome}; {elapsed:.2f} s; peak RSS +{growth / MIB:.1f} MiB')
        assert request(port)[0] == 200, 'the server stops serving after the attack'
    assert outcome in passing
    assert elapsed < seconds
    assert growth < RSS_BOUND
