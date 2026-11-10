#!/usr/bin/env python3
"""Run unchanged PHP applications against one or more Swoole implementations."""
import argparse
import base64
import concurrent.futures
from datetime import datetime, timezone
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.harness import ROOT, parse_engines, request, server, write_report, run_isolated
from corpus.extra_projects import TESTS as EXTRA_TESTS

MANIFEST = json.loads((ROOT / 'corpus/manifest.json').read_text())
AUTH = {'Authorization': 'Bearer corpus-secret', 'Content-Type': 'application/json'}


def expect(actual, expected):
    if actual != expected:
        raise AssertionError(f'expected {expected!r}, got {actual!r}')


def api(port, path, method='GET', data=None, status=200, headers=AUTH):
    result = request(port, path, method, json.dumps(data) if data is not None else None, headers)
    expect(result[0], status)
    if status == 204:
        expect(result[2], b'')
        return None
    return json.loads(result[2])


def hello(port):
    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
    try:
        first_socket = None
        for _ in range(3):
            conn.request('GET', '/')
            response = conn.getresponse()
            expect(response.status, 200)
            expect(response.getheader('content-type'), 'text/plain')
            expect(response.read(), b'Hello, Swoole!\n')
            if conn.sock is None:
                raise AssertionError('keep-alive connection closed')
            if first_socket is None:
                first_socket = conn.sock
            expect(conn.sock, first_socket)
    finally:
        conn.close()


def router(port):
    payload = bytes(range(256)) * 256
    expect(request(port, '/echo', 'POST', payload)[::2], (200, payload))
    expect(api(port, '/inspect?q=hello+world&tags%5B%5D=a&tags%5B%5D=b',
               headers={'X-Token': 'test-token'}),
           {'query': {'q': 'hello world', 'tags': ['a', 'b']}, 'token': 'test-token'})
    api(port, '/missing', status=404)
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
        for body in pool.map(lambda _: request(port, '/echo', 'POST', b'parallel')[2], range(32)):
            expect(body, b'parallel')


def notes(port):
    api(port, '/notes', status=401, headers={})
    api(port, '/notes', 'POST', {'title': ''}, status=422)
    note = api(port, '/notes', 'POST', {'title': 'first note'}, status=201)
    expect(note, {'id': 1, 'title': 'first note'})
    expect(api(port, '/notes/1'), note)
    expect(api(port, '/notes'), [note])
    api(port, '/notes/1', 'PUT', {}, status=405)
    api(port, '/notes/1', 'DELETE', status=204)
    api(port, '/notes/1', status=404)
    expect(api(port, '/notes'), [])


def jobs(port):
    expect(api(port, '/health', headers={}), {'ok': True})
    api(port, '/jobs', 'POST', {'payload': {}}, status=401, headers={})
    api(port, '/jobs', 'POST', {'bad': 'input'}, status=422)
    job = api(port, '/jobs', 'POST', {'payload': {'to': 'example', 'body': 'hello'}}, status=201)
    expect(job['status'], 'queued')
    expect(api(port, f'/jobs/{job["id"]}'), job)
    completed = api(port, f'/jobs/{job["id"]}/complete', 'POST')
    expect(completed, {**job, 'status': 'completed'})
    expect(api(port, f'/jobs/{job["id"]}/complete', 'POST'), completed)
    expect(api(port, '/metrics'), {'completed': 1})
    api(port, '/jobs/999999', status=404)


def coroutines(engine, entry):
    result = run_isolated(engine.command(entry), timeout=10)
    try:
        expect(result.returncode, 0)
        expect(json.loads(result.stdout), {'results': [{'id': i, 'result': i*i} for i in range(1, 5)], 'timed_out': True})
    except (AssertionError, ValueError) as error:
        raise AssertionError(f'{error}; stdout={result.stdout[-1500:]!r}; stderr={result.stderr[-1500:]!r}') from error


class WebSocket:
    def __init__(self, port, deflate=False, tls=None):
        """`deflate` negotiates permessage-deflate without context takeover, as Swoole offers it:
        each message is compressed on its own. `tls` is an ssl.SSLContext to connect with (wss)."""
        self.socket = socket.create_connection(('127.0.0.1', port), timeout=5)
        if tls:
            self.socket = tls.wrap_socket(self.socket, server_hostname='127.0.0.1')
        self.deflate = deflate
        try:
            key = base64.b64encode(os.urandom(16)).decode()
            extension = ('Sec-WebSocket-Extensions: permessage-deflate; client_no_context_takeover; '
                         'server_no_context_takeover\r\n' if deflate else '')
            self.socket.sendall((f'GET /chat HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n'
                                 f'Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n'
                                 f'{extension}Sec-WebSocket-Key: {key}\r\n\r\n').encode())
            response = bytearray()
            while not response.endswith(b'\r\n\r\n'):
                response.extend(self.read(1))
                if len(response) > 16384:
                    raise AssertionError('oversized WebSocket handshake')
            lines = response.decode().split('\r\n')
            expect(lines[0].split(' ', 2)[1], '101')
            headers = {key.lower(): value.strip() for key, value in (line.split(':', 1) for line in lines[1:] if ':' in line)}
            accept = base64.b64encode(hashlib.sha1((key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
            expect(headers['sec-websocket-accept'], accept)
            if deflate:
                expect(headers.get('sec-websocket-extensions', '').split(';')[0], 'permessage-deflate')
        except BaseException:
            self.socket.close()
            raise

    def read(self, length):
        data = bytearray()
        while len(data) < length:
            chunk = self.socket.recv(length - len(data))
            if not chunk:
                raise AssertionError('unexpected WebSocket EOF')
            data.extend(chunk)
        return bytes(data)

    def send(self, payload, opcode=1, fin=True):
        """One frame: `fin=False` and then opcode 0 send a message in fragments."""
        mask = os.urandom(4)
        first = (0x80 if fin else 0) | opcode
        if self.deflate:
            compressor = zlib.compressobj(wbits=-15)
            payload = (compressor.compress(payload) + compressor.flush(zlib.Z_SYNC_FLUSH))[:-4]
            first |= 0x40  # RSV1: compressed
        if len(payload) < 126:
            length = bytes([0x80 | len(payload)])
        elif len(payload) < 65536:
            length = b'\xfe' + struct.pack('!H', len(payload))
        else:
            length = b'\xff' + struct.pack('!Q', len(payload))
        key = int.from_bytes(mask * ((len(payload) + 3) // 4), 'big') >> (-len(payload) % 4 * 8)
        masked = (int.from_bytes(payload, 'big') ^ key).to_bytes(len(payload), 'big')
        self.socket.sendall(bytes([first]) + length + mask + masked)

    def receive(self):
        first, second = self.read(2)
        compressed = self.deflate and first & 0x40
        expect(first & ~0x40 if compressed else first, 0x81)
        expect(second & 0x80, 0)
        length = second & 0x7f
        if length == 126:
            length = struct.unpack('!H', self.read(2))[0]
        elif length == 127:
            length = struct.unpack('!Q', self.read(8))[0]
        if length > 1024 * 1024:
            raise AssertionError('oversized test frame')
        payload = self.read(length)
        return zlib.decompressobj(wbits=-15).decompress(payload + b'\x00\x00\xff\xff') if compressed else payload

    def close(self):
        self.socket.close()


def websocket(port):
    first = WebSocket(port)
    try:
        second = WebSocket(port)
        try:
            first.send(b'hello room')
            expect(first.receive(), b'hello room')
            expect(second.receive(), b'hello room')
            # Complete the closing handshake before asserting another broadcast.
            second.send(b'\x03\xe8', opcode=8)
            header = second.read(2)
            expect(header, b'\x88\x02')  # FIN, CLOSE, unmasked, two-byte status.
            expect(second.read(2), b'\x03\xe8')
            expect(second.socket.recv(1), b'')
        finally:
            second.close()
        first.send(b'still here')
        expect(first.receive(), b'still here')
    finally:
        first.close()


TESTS = {'hello': hello, 'router': router, 'notes': notes, 'jobs': jobs, 'websocket': websocket, 'coroutines': coroutines, **EXTRA_TESTS}


def run_project(engine, project, artifacts=None):
    entry = (ROOT / 'corpus' / project['id'] / project['entry']).resolve()
    if project.get('mode') == 'driver':
        artifacts.mkdir(parents=True, exist_ok=True)
        output = artifacts / f'{engine.name}-{project["id"]}.json'
        command = [sys.executable, str(entry), '--engine', f'{engine.name}={engine.extension}',
                   '--php', engine.php, '--output', str(output)]
        for extension in engine.php_extensions:
            command += ['--php-extension', extension]
        result = run_isolated(command, timeout=project.get('timeout_seconds', 120))
        output.with_suffix('.log').write_text(result.stdout + result.stderr)
        if result.returncode:
            raise AssertionError(f'driver exited {result.returncode}; evidence={output}; '
                                 f'{(result.stdout + result.stderr)[-1500:]}')
        return str(output)
    elif project.get('mode') == 'cli':
        TESTS[project['test']](engine, entry)
    elif project['test'] == 'jobs':
        # Real disk state must survive stopping and starting the whole service.
        with tempfile.TemporaryDirectory(prefix='swoole-jobs-') as data:
            with server(engine, entry, {'TEST_DATA': data}) as (port, _):
                jobs(port)
            with server(engine, entry, {'TEST_DATA': data}) as (port, _):
                expect(api(port, '/jobs/1')['status'], 'completed')
                expect(api(port, '/metrics'), {'completed': 1})
    else:
        with server(engine, entry) as (port, _):
            TESTS[project['test']](port)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', required=True, help='NAME=/path/to/extension; repeat to compare')
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[], help='Extra PHP module to load before Swoole; repeat as needed')
    parser.add_argument('--project', action='append', choices=[p['id'] for p in MANIFEST])
    parser.add_argument('--require-all', action='store_true', help='Fail on unsupported capabilities too')
    parser.add_argument('--output', default=str(ROOT / 'target/reports/corpus.json'))
    args = parser.parse_args()
    engines = parse_engines(args.engine, args.php, args.php_extension)
    report = {'schema_version': 1, 'created_utc': datetime.now(timezone.utc).isoformat(), 'engines': [], 'results': []}
    for engine in engines:
        metadata = engine.metadata()
        report['engines'].append(metadata)
        for project in MANIFEST:
            if args.project and project['id'] not in args.project:
                continue
            missing = sorted(set(project['requires']) - set(metadata['capabilities']))
            row = {'engine': engine.name, 'project': project['id']}
            started = time.monotonic()
            if missing:
                row.update(status='unsupported', detail=f'missing capabilities: {", ".join(missing)}')
            else:
                try:
                    evidence = run_project(engine, project, Path(args.output).resolve().parent / (Path(args.output).stem + '-artifacts'))
                    row.update(status='passed')
                    if evidence:
                        row['evidence'] = evidence
                except Exception as error:
                    row.update(status='failed', detail=f'{type(error).__name__}: {error}')
            row['seconds'] = round(time.monotonic() - started, 4)
            report['results'].append(row)
            print(f'{engine.name:12} {project["id"]:20} {row["status"]:12} {row.get("detail", "")}', flush=True)
    write_report(args.output, report)
    print(f'Report: {args.output}')
    return int(any(row['status'] == 'failed' or args.require_all and row['status'] == 'unsupported' for row in report['results']))


if __name__ == '__main__':
    sys.exit(main())
