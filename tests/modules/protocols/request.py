#!/usr/bin/env python3
"""Actual Redis server typed callback decoding, independent of client packages."""
import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import socket
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from tools.harness import ROOT, parse_engines, server, write_report


def read(peer, count):
    output = bytearray()
    while len(output) < count:
        chunk = peer.recv(count - len(output))
        assert chunk, f'EOF after {len(output)}/{count} bytes'
        output.extend(chunk)
    return bytes(output)


def bulk(value):
    return b'$' + str(len(value)).encode() + b'\r\n' + value + b'\r\n'


def cases():
    # The original receiver counts bulk strings, excluding nulls and integers.
    # A final bulk ensures those parameters arrive before dispatch even on split TCP.
    tail = b'tail'
    values = [
        ('empty', [b'']), ('binary', [b'a\0b\r\nc']),
        ('null', [None]), ('integer', [42, -1, 2147483648]),
        ('mixed', [None, b'', -3, b'binary\0', None]),
        ('many', [str(i).encode() for i in range(97)]),
        ('large', [b'x\0' * 1048576]),
    ]
    commands = [(b'ECHO', 'ECHO'), (b'mIxEd', 'MiXeD'), (b'l' * 63, 'l' * 63),
                (b'ECHO\0ignored', 'ECHO')]
    for command, label in commands:
        for name, params in values:
            all_params = [*params, tail]
            fields = [bulk(command)]
            typed = []
            for value in all_params:
                if value is None:
                    fields.append(b'$-1\r\n')
                    typed.append(['null', None])
                elif isinstance(value, int):
                    fields.append(b':' + str(value).encode() + b'\r\n')
                    typed.append(['int', (value + 2**31) % 2**32 - 2**31])
                else:
                    fields.append(bulk(value))
                    typed.append(['string', base64.b64encode(value).decode()])
            count = 1 + sum(isinstance(value, bytes) for value in all_params)
            yield command.hex() + '-' + name, b'*' + str(count).encode() + b'\r\n' + b''.join(fields), [label, typed]
    for prefix, expected in [(b':+17\r\n', ['int', 17]), (b'$-1\r\n', ['null', None])]:
        yield 'before-command-' + expected[0], b'*2\r\n' + prefix + bulk(b'ECHO') + bulk(tail), ['ECHO', [expected, ['string', base64.b64encode(tail).decode()]]]
    for index, (number, expected) in enumerate([
        (b'  +00017', 17), (b'\t-0002', -2),
        (b'9223372036854775808', -1), (b'-9223372036854775809', 0),
        (b'999999999999999999999999999999999999', -1),
    ]):
        wire = b'*2\r\n' + bulk(b'ECHO') + b':' + number + b'\r\n' + bulk(tail)
        yield 'number-' + str(index), wire, ['ECHO', [['int', expected], ['string', base64.b64encode(tail).decode()]]]


def run(engine, mode, coroutines, output):
    env = {'TEST_SERVER_MODE': mode, 'TEST_COROUTINES': str(int(coroutines))}
    with server(engine, Path(__file__).with_suffix('.php'), env) as (port, _):
        for name, wire, expected in cases():
            row = {'case': name, 'request_sha256': hashlib.sha256(wire).hexdigest(), 'passed': False}
            output.append(row)
            with socket.create_connection(('127.0.0.1', port), timeout=5) as peer:
                peer.settimeout(5)
                peer.sendall(wire)
                header = b''
                while not header.endswith(b'\r\n'):
                    header += read(peer, 1)
                    assert len(header) < 32, (name, header)
                assert header.startswith(b'$'), (name, header)
                length = int(header[1:-2])
                body = read(peer, length)
                assert read(peer, 2) == b'\r\n', name
                assert json.loads(body) == expected, name
            row.update(reply_sha256=hashlib.sha256(body).hexdigest(), passed=True)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='PHP module loaded before Swoole')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/reports/protocols-request.json')
    args = parser.parse_args()
    report = {'created_utc': datetime.now(timezone.utc).isoformat(), 'engines': [], 'results': [],
              'scope': 'Actual native PHP request dispatcher: null/int order, first string command, routing, binary/empty strings, overflow and 2MiB payloads; no optional clients.'}
    for engine in parse_engines(args.engine, args.php, args.php_extension):
        report['engines'].append(engine.metadata())
        for mode, coroutines in [('base', False), ('base', True), ('process', False), ('process', True)]:
            row = {'engine': engine.name, 'mode': mode, 'coroutines': coroutines, 'passed': False, 'cases': []}
            try:
                run(engine, mode, coroutines, row['cases'])
                row['passed'] = True
            except (AssertionError, OSError, RuntimeError, ValueError) as error:
                row['error'] = f'{type(error).__name__}: {error}'
            report['results'].append(row)
            write_report(args.output, report)
    print(json.dumps({'passed': sum(row['passed'] for row in report['results']), 'groups': len(report['results'])}))
    return int(any(not row['passed'] for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
