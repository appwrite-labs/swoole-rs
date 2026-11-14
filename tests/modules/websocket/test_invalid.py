"""Pinned Swoole malformed/control/aggregate network-policy compatibility gate."""
import contextlib
from pathlib import Path
import struct

import pytest

from corpus.run import WebSocket, expect
from tools.harness import request, server
from test_close import frame


def wire(payload, opcode, final=True, masked=True):
    if masked:
        return frame(payload, opcode, final)
    size = len(payload)
    header = bytes([(0x80 if final else 0) | opcode])
    return header + (bytes([size]) if size < 126 else b'\x7e' + struct.pack('!H', size)) + payload


def scenarios():
    rows = [('unexpected-continuation', frame(b'orphan', opcode=0), None),
            ('second-fragment-start', frame(b'first', final=False) + frame(b'second', final=False), None),
            ('fragmented-ping-permitted', frame(b'P', opcode=9, final=False), (b'\x8a\x01', b'P'))]
    # Pinned policy compares total frame bytes with 127, not payload with 125.
    for masked, lengths in [(True, [120, 121, 125, 126]), (False, [124, 125, 126])]:
        for opcode in [9, 10]:
            for length in lengths:
                payload = b'c' * length
                accepted = length + (6 if masked else 2) < 127 and length < 126
                response = (bytes([0x8a, length]), payload) if opcode == 9 and accepted else () if accepted else None
                rows.append((f'{"masked" if masked else "unmasked"}-{"ping" if opcode == 9 else "pong"}-{length}',
                             wire(payload, opcode, masked=masked), response))
    for total in [65535, 65536, 65537]:
        payload = b'a' * 32768 + b'b' * (total - 32768)
        rows.append((f'fragment-aggregate-{total}', frame(payload[:32768], final=False) + frame(payload[32768:], opcode=0),
                     ('echo', payload) if total <= 65536 else None))
    return rows


def run_case(port, name, data, expected):
    client = WebSocket(port)
    row = {'case': name, 'sent_bytes': len(data),
           'expected': 'connection-terminated' if expected is None else 'accepted', 'passed': False}
    try:
        client.socket.settimeout(2)
        client.socket.sendall(data)
        if expected is None:
            try:
                expect(client.socket.recv(1), b'')
                row['observed'] = 'eof'
            except ConnectionResetError:
                row['observed'] = 'reset'
        else:
            if expected and expected[0] == 'echo':
                expect(client.receive(), expected[1])
            elif expected:
                expect(client.read(2), expected[0])
                expect(client.read(len(expected[1])), expected[1])
            # This reply is a barrier: accepted pong/control cannot be mistaken for silence.
            client.send(b'after-policy')
            expect(client.receive(), b'after-policy')
            client.send(b'\x03\xe8', opcode=8)
            expect(client.read(4), b'\x88\x02\x03\xe8')
            expect(client.socket.recv(1), b'')
            row['observed'] = 'reply-barrier-and-close-eof'
        row['passed'] = True
    except Exception as error:
        row['error'] = f'{type(error).__name__}: {error}'
    finally:
        client.close()
    try:
        status, _, body = request(port)
        expect(status, 200)
        expect(body, b'healthy')
        row['health_after'] = True
    except Exception as error:
        row['passed'] = False
        row['health_error'] = f'{type(error).__name__}: {error}'
    return row


@pytest.fixture(scope='module')
def ports():
    """One server per engine for all of its cases, as the pinned gate ran them."""
    with contextlib.ExitStack() as stack:
        started = {}

        def port(engine):
            if engine.name not in started:
                started[engine.name] = stack.enter_context(server(engine, Path(__file__).with_name('invalid.php')))[0]
            return started[engine.name]
        yield port


@pytest.mark.parametrize('name,data,expected', scenarios(), ids=[row[0] for row in scenarios()])
def test_invalid(engine, ports, name, data, expected):
    """Explicit pinned policy, not RFC conformance. Each case uses a fresh socket;
    rejected connections must terminate (EOF/reset), a timeout fails. Accepted controls
    have a text reply barrier, and HTTP health is checked after every case. Aggregate
    payload limit 65536 (native minimum)."""
    assert len(scenarios()) == 20
    row = run_case(ports(engine), name, data, expected)
    assert row['passed'], row
