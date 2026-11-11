#!/usr/bin/env python3
"""Instructions per request for benchmark workloads, counted by callgrind.

Instruction counts barely vary between runs of the same binary, so Cargo build/upstream
ratios are stable on shared CI runners where timings are not. Only requests are
counted: instrumentation is switched on after startup and warmup, and off before
shutdown. Every process (master, manager, worker) is followed and summed.
With two engines and --max-ratio, exits 1 if any workload's first/second ratio exceeds it.
--limits is the merge tier's ratchet: a directory with one file per workload holding its
recorded ratio (so PRs changing different records never conflict), each allowed to drift by
NOISE (docs/migration.md, Speed). --ratchet lowers those records to what two consecutive
merge-tier results both beat (.github/workflows/badges.yml files the issue).
"""
import argparse
import gzip
import http.client
import http.server
import json
import math
import os
from pathlib import Path
import shutil
import signal
import socket
import socketserver
import ssl
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools.annotations import annotate
from tools.harness import ROOT, parse_engines
from corpus.run import WebSocket

NOISE = 1.01  # counts for the same binaries drift well under 1% between runs
PAYLOAD = bytes(range(256)) * 256
# Compressible JSON text, about 4 KiB and 1 KiB.
DOCUMENT = json.dumps([{'id': i, 'event': 'order.created', 'status': 'paid', 'total': i * 25}
                       for i in range(64)], separators=(',', ':')).encode()
MESSAGE = DOCUMENT[:1024]
FRAME = (1024).to_bytes(4, 'big') + PAYLOAD[:1024]
# (application directory, method, path, body, expected response body). method 'WS' opens a
# WebSocket to the app and echoes `body` frame by frame instead of sending HTTP; 'WSZ' does
# so with permessage-deflate. 'GZ' is a POST sent with Accept-Encoding: gzip whose response
# must come back gzip-encoded. 'TCP' and 'UDP' send `body` as raw bytes on one connection
# (one socket) and read `expected` back. 'TLS' is a GET over HTTPS on one connection and
# 'WSS' the WebSocket echo over TLS. The pin's test certificate is self-signed, so the client doesn't
# verify it.
# 'H2C' posts `body` on 16 HTTP/2 streams at once per request; 'WSF' sends `body` as a WebSocket
# message in four fragments; 'UP' posts `body` as a multipart file upload.
# 'GET1' is a GET on a new connection each time, closed by the server (Connection: close).
H2BODY = PAYLOAD[:4000]
STATIC = (Path(__file__).parent / 'traffic/static.txt').read_bytes()
BOUNDARY = 'swoole-rs-benchmark'
WORKLOADS = {
    'plaintext': ('corpus/01-hello', 'GET', '/', None, b'Hello, Swoole!\n'),
    'json': ('corpus/02-router', 'GET', '/inspect?q=benchmark', None,
             b'{"query":{"q":"benchmark"},"token":null}'),
    'echo-64k': ('corpus/02-router', 'POST', '/echo', PAYLOAD, PAYLOAD),
    'echo-1k': ('corpus/02-router', 'POST', '/echo', PAYLOAD[:1024], PAYLOAD[:1024]),
    'service-health': ('corpus/04-job-service', 'GET', '/health', None, b'{"ok":true}'),
    # Graduation gates for the websocket (#117) and timer (#118) modules: the workload
    # that exercises each most, held to --max-ratio 1.05 rather than the ratchet.
    'websocket': ('corpus/06-websocket', 'WS', '/chat', b'ping', b'ping'),
    'timer': ('corpus/08-timers', 'GET', '/', None, b'timers\n'),
    # Routing, JSON, validation, a Redis pool, a file read and three coroutines per request
    # (#1907); needs Redis on REDIS_HOST:REDIS_PORT, default 127.0.0.1:6379.
    'app': ('benchmarks/app', 'POST', '/users/42/orders', b'{"sku":"A-1","quantity":3,"note":"gift"}',
            b'{"user":42,"sku":"A-1","quantity":3,"total":750,"in_stock":true,"stored":true}'),
    # Runtime primitives (#1909). method 'H2' sends the request as HTTP/2 with prior knowledge.
    'http2': ('benchmarks/primitives', 'H2', '/', None, b'ok\n'),
    'table': ('benchmarks/primitives', 'GET', '/table', None, b'2016\n'),
    'channel': ('benchmarks/primitives', 'GET', '/channel', None, b'2016\n'),
    'task': ('corpus/10-task-workers', 'POST', '/', b'[1,2,3]', b'{"sum":6,"count":3,"task_worker":true}'),
    # A prepared SELECT through hooked PDO mysql (#2066); needs MySQL on MYSQL_HOST (root/root,
    # database test) and --php-extension mysqlnd --php-extension pdo --php-extension pdo_mysql.
    'mysql': ('benchmarks/mysql', 'GET', '/?id=42', None, b'{"id":42,"name":"benchmark"}'),
    # plaintext with accept, the connection's setup and its close in every request (#2651).
    'connect': ('corpus/01-hello', 'GET1', '/', None, b'Hello, Swoole!\n'),
    # Outbound coroutine clients (#2647), against the peers below.
    'http-client': ('benchmarks/clients', 'GET', '/http', None, b'ok\n'),
    'client': ('benchmarks/clients', 'GET', '/client', None, b'ok\n'),
    'socket': ('benchmarks/clients', 'GET', '/socket', None, b'ok\n'),
    # Compression (#2649): a gzip-encoded HTTP response and a permessage-deflate echo.
    'gzip': ('benchmarks/compression', 'GZ', '/gzip', DOCUMENT, DOCUMENT),
    'websocket-deflate': ('benchmarks/compression', 'WSZ', '/chat', MESSAGE, MESSAGE),
    # Swoole\Server without HTTP (#2650): length-prefixed and end-marker framing, and UDP.
    'tcp-length': ('corpus/07-tcp-framing', 'TCP', None, FRAME, FRAME),
    'tcp-eof': ('benchmarks/framing', 'TCP', None, PAYLOAD[:1024] + b'\r\n', PAYLOAD[:1024] + b'\r\n'),
    'udp': ('benchmarks/framing', 'UDP', None, PAYLOAD[:1024], PAYLOAD[:1024]),
    # TLS (#2648): the record layer on a kept-alive connection, and wss.
    'https': ('benchmarks/tls', 'TLS', '/', None, b'ok\n'),
    'wss': ('benchmarks/tls', 'WSS', '/chat', b'ping', b'ping'),
    # Swoole\Atomic and Swoole\Lock, and plaintext dispatched through SWOOLE_PROCESS's master (#2654).
    'atomic': ('benchmarks/primitives', 'GET', '/atomic', None, b'2016\n'),
    'process-mode': ('benchmarks/process', 'GET', '/', None, b'Hello, Swoole!\n'),
    # Larger and concurrent traffic (#2652), against benchmarks/traffic.
    'http2-concurrent': ('benchmarks/traffic', 'H2C', '/echo', H2BODY, H2BODY),
    'websocket-64k': ('benchmarks/traffic', 'WS', '/', PAYLOAD, PAYLOAD),
    'websocket-fragmented': ('benchmarks/traffic', 'WSF', '/', PAYLOAD[:4096], PAYLOAD[:4096]),
    'upload': ('benchmarks/traffic', 'UP', '/upload', PAYLOAD, b'65536\n'),
    'static': ('benchmarks/traffic', 'GET', '/static.txt', None, STATIC),
    # Database and network hooks (#2653), against benchmarks/hooks on the build with
    # --enable-swoole-pgsql, --enable-swoole-sqlite, --enable-swoole-curl and --enable-cares.
    # pgsql needs PostgreSQL on PGSQL_HOST (root/root, database test) and --php-extension
    # pdo_pgsql, sqlite --php-extension pdo_sqlite, curl --php-extension curl.
    'pgsql': ('benchmarks/hooks', 'GET', '/pgsql', None, b'ok\n'),
    'sqlite': ('benchmarks/hooks', 'GET', '/sqlite', None, b'ok\n'),
    'curl': ('benchmarks/hooks', 'GET', '/curl', None, b'ok\n'),
    'dns': ('benchmarks/hooks', 'GET', '/dns', None, b'ok\n'),
}
TLS = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
TLS.check_hostname, TLS.verify_mode = False, ssl.CERT_NONE


class Pong(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def do_GET(self):
        self.send_response(200)
        self.send_header('Content-Length', '4')
        self.end_headers()
        self.wfile.write(b'pong')

    def log_message(self, *args):
        pass


class Echo(socketserver.BaseRequestHandler):
    # The reply waits 2 ms, so the client's recv() always finds nothing and waits on the
    # reactor once. Whether a prompt reply beat recv() was timing, and swung client and socket
    # between 0.86 and 1.05 from one merge run to the next.
    def handle(self):
        while data := self.request.recv(65536):
            time.sleep(0.002)
            self.request.sendall(data)


class Resolver(socketserver.BaseRequestHandler):
    """Answers every DNS query with one A record, 127.0.0.1."""
    def handle(self):
        query, sock = self.request
        end = 12
        while query[end]:
            end += query[end] + 1
        # The query's ID; QR, RD and RA, no error; one question (the query's: its name, root
        # label, type and class) and one answer: the question's name, A, IN, TTL 60, 127.0.0.1.
        sock.sendto(query[:2] + bytes.fromhex('8180 0001 0001 0000 0000') + query[12:end + 5]
                    + bytes.fromhex('c00c 0001 0001 0000003c 0004 7f000001'), self.client_address)


def peers():
    """Environment naming the peers benchmarks/clients and benchmarks/hooks call: an HTTP server,
    a TCP echo and a DNS resolver, served from threads of this process so callgrind counts only
    the client side."""
    if not hasattr(peers, 'env'):
        servers = [http.server.ThreadingHTTPServer(('127.0.0.1', 0), Pong),
                   socketserver.ThreadingTCPServer(('127.0.0.1', 0), Echo),
                   socketserver.ThreadingUDPServer(('127.0.0.1', 0), Resolver)]
        for server in servers:
            server.daemon_threads = True
            threading.Thread(target=server.serve_forever, daemon=True).start()
        peers.env = {f'PEER_{name}_PORT': str(server.server_address[1])
                     for name, server in zip(('HTTP', 'ECHO', 'DNS'), servers)}
    return peers.env


def h2_frame(kind, flags, stream, payload=b''):
    return len(payload).to_bytes(3, 'big') + bytes([kind, flags]) + stream.to_bytes(4, 'big') + payload


def send_h2(port, path, expected, count, body=None, streams=1):
    """`count` rounds over one HTTP/2 connection, each opening `streams` streams at once: a GET of
    `path`, or a POST of `body` when there is one, and reading every stream's response."""
    with socket.create_connection(('127.0.0.1', port), timeout=60) as sock:
        reader = sock.makefile('rb')
        sock.sendall(b'PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n' + h2_frame(4, 0, 0))
        # HPACK: :method GET (or POST), :scheme http, then :path and :authority as literals of
        # static-table names, never indexed, so the encoder keeps no state.
        block = (b'\x83' if body else b'\x82') + b'\x86\x04' + bytes([len(path)]) + path.encode() + b'\x01\x09localhost'
        stream = 1
        for _ in range(count):
            bodies = {}
            for _ in range(streams):
                if body:
                    sock.sendall(h2_frame(1, 0x4, stream, block) + h2_frame(0, 0x1, stream, body))  # END_HEADERS; END_STREAM
                else:
                    sock.sendall(h2_frame(1, 0x5, stream, block))  # END_STREAM | END_HEADERS
                bodies[stream] = b''
                stream += 2
            open_streams = set(bodies)
            while open_streams:
                header = reader.read(9)
                length, kind, flags = int.from_bytes(header[:3], 'big'), header[3], header[4]
                sid = int.from_bytes(header[5:9], 'big') & 0x7fffffff
                payload = reader.read(length)
                if kind == 4 and not flags & 1:
                    sock.sendall(h2_frame(4, 1, 0))  # SETTINGS ACK
                elif kind == 7:
                    raise RuntimeError(f'GOAWAY: {payload!r}')
                elif kind == 1 and payload[:1] != b'\x88':  # :status 200, static index 8
                    raise RuntimeError(f'unexpected HTTP/2 response headers: {payload!r}')
                elif kind == 0 and length:
                    bodies[sid] += payload
                    sock.sendall(h2_frame(8, 0, 0, length.to_bytes(4, 'big')))  # WINDOW_UPDATE
                if kind in (0, 1) and flags & 1:
                    open_streams.discard(sid)
            for received in bodies.values():
                if received != expected:
                    raise RuntimeError(f'unexpected HTTP/2 body: {received[:80]!r}')


def wait_for_port(port, proc, seconds=180):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f'server exited during startup ({proc.returncode})')
        try:
            socket.create_connection(('127.0.0.1', port), timeout=1).close()
            return
        except OSError:
            time.sleep(0.5)
    raise RuntimeError('server did not listen in time')


def send(port, workload, count):
    _, method, path, payload, expected = workload
    if method in ('WS', 'WSF', 'WSZ', 'WSS'):
        client = WebSocket(port, deflate=method == 'WSZ', tls=TLS if method == 'WSS' else None)
        try:
            for _ in range(count):
                if method == 'WSF':
                    quarter = len(payload) // 4
                    for i in range(4):
                        client.send(payload[i * quarter:(i + 1) * quarter], opcode=0 if i else 1, fin=i == 3)
                else:
                    client.send(payload)
                body = client.receive()
                if body != expected:
                    raise RuntimeError(f'unexpected WebSocket frame: {body[:80]!r}')
        finally:
            client.close()
        return
    if method in ('H2', 'H2C'):
        send_h2(port, path, expected, count, *((payload, 16) if method == 'H2C' else ()))
        return
    if method == 'GET1':
        request = f'GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n'.encode()
        for _ in range(count):
            # A raw socket read to EOF: http.client drops its socket on Connection: close and
            # stops at Content-Length, so the server's close could fall after the request.
            with socket.create_connection(('127.0.0.1', port), timeout=60) as sock:
                sock.sendall(request)
                reply = b''
                while chunk := sock.recv(65536):
                    reply += chunk
            head, _, body = reply.partition(b'\r\n\r\n')
            if not head.startswith(b'HTTP/1.1 200 ') or body != expected:
                raise RuntimeError(f'unexpected response: {reply[:80]!r}')
        return
    if method in ('TCP', 'UDP'):
        if method == 'TCP':
            sock = socket.create_connection(('127.0.0.1', port), timeout=60)
        else:
            sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            sock.settimeout(60)
            sock.connect(('127.0.0.1', port))
        with sock:
            for _ in range(count):
                sock.sendall(payload)
                body = sock.recv(65536)
                while method == 'TCP' and len(body) < len(expected) and (chunk := sock.recv(65536)):
                    body += chunk
                if body != expected:
                    raise RuntimeError(f'unexpected {method} reply: {body[:80]!r}')
        return
    if method == 'TLS':
        conn = http.client.HTTPSConnection('127.0.0.1', port, timeout=60, context=TLS)
    else:
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=60)
    for _ in range(count):
        if method == 'GZ':
            conn.request('POST', path, payload, {'Accept-Encoding': 'gzip'})
        elif method == 'UP':
            form = (f'--{BOUNDARY}\r\nContent-Disposition: form-data; name="file"; filename="upload.bin"\r\n'
                    f'Content-Type: application/octet-stream\r\n\r\n').encode() + payload + f'\r\n--{BOUNDARY}--\r\n'.encode()
            conn.request('POST', path, form, {'Content-Type': f'multipart/form-data; boundary={BOUNDARY}'})
        else:
            conn.request('GET' if method == 'TLS' else method, path, payload)
        response = conn.getresponse()
        body = response.read()
        if method == 'GZ':
            if response.getheader('Content-Encoding') != 'gzip':
                raise RuntimeError(f'response not gzip-encoded: {response.getheaders()!r}')
            body = gzip.decompress(body)
        if response.status not in (200, 201) or body != expected:
            raise RuntimeError(f'unexpected response {response.status}: {body[:80]!r}')
    conn.close()


def instructions(engine, workload, requests, warmup, profiles=None):
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    with tempfile.TemporaryDirectory(prefix='swr-callgrind-') as out, tempfile.TemporaryDirectory() as data:
        command = ['valgrind', '--tool=callgrind', '--trace-children=yes', '--instr-atstart=no',
                   f'--callgrind-out-file={out}/cg.%p',
                   *engine.command(ROOT / workload[0] / 'server.php')]
        log = Path(out) / 'server.log'
        with log.open('wb') as sink:
            proc = subprocess.Popen(command, env={**os.environ, **peers(), 'TEST_PORT': str(port), 'TEST_DATA': data},
                                    stdout=sink, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            wait_for_port(port, proc)
            try:
                send(port, workload, warmup)
            except Exception:
                # The server's own words say why it closed or answered wrongly.
                print(log.read_text(errors='replace')[-4000:], file=sys.stderr, flush=True)
                raise
            subprocess.run(['callgrind_control', '-i', 'on'], check=True, capture_output=True)
            send(port, workload, requests)
            subprocess.run(['callgrind_control', '-i', 'off'], check=True, capture_output=True)
            # Dump now: a signalled valgrind may exit without writing its profile.
            subprocess.run(['callgrind_control', '-d'], check=True, capture_output=True)
        finally:
            os.killpg(proc.pid, signal.SIGTERM)
            proc.wait(timeout=120)
        total = 0
        if profiles:
            shutil.copytree(out, profiles / engine.name, dirs_exist_ok=True)
        for path in Path(out).glob('cg.*'):
            for line in path.read_text(errors='replace').splitlines():
                if line.startswith('totals:'):
                    total += int(line.split()[1])
        return total / requests


def verdict(ratio, recorded=None, max_ratio=None):
    """(limit, over, stale) for one workload's ratio: stale when it beats `recorded` by
    more than noise, so the ratchet should be lowered."""
    limit = recorded * NOISE if recorded else max_ratio
    return limit, bool(limit and ratio > limit), bool(recorded and ratio < recorded / NOISE)


def lowest(ratio):
    """The record a workload's ratio could be lowered to: rounded up to two decimals."""
    return math.ceil(round(ratio * 100, 6)) / 100


def read_limits(directory):
    """{workload: recorded ratio} from a limits directory: one file per workload, holding its ratio."""
    return {path.name: float(path.read_text()) for path in sorted(directory.iterdir())}


def ratchet(results, limits):
    """{workload: lower record} for each recorded workload that every result beats once rounded up.
    A result from before the Cargo build replaced the hybrid (no `cargo` row) lowers nothing."""
    lowered = {}
    for workload, limit in limits.items():
        rows = [result['instructions_per_request'].get(workload) for result in results]
        if results and all(row and 'cargo' in row for row in rows):
            record = max(lowest(row['cargo'] / row['upstream']) for row in rows)
            if record < limit:
                lowered[workload] = record
    return lowered


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--engine', action='append', help='NAME=/path/to/extension')
    parser.add_argument('--workload', choices=list(WORKLOADS), action='append')
    parser.add_argument('--requests', type=int, default=2000)
    parser.add_argument('--warmup', type=int, default=100)
    parser.add_argument('--php', default='php')
    parser.add_argument('--php-extension', action='append', default=[])
    # Large output stays in this worktree's target/, where dev.py prune finds it (#395, #396).
    run = ROOT / 'target' / 'callgrind' / time.strftime('%Y%m%dT%H%M%S')
    parser.add_argument('--output', type=Path, default=run / 'callgrind.json')
    parser.add_argument('--profiles', type=Path, nargs='?', const=run / 'profiles',
                        help='Keep the callgrind profiles in DIR/<workload>/<engine>/ (default under target/callgrind/)')
    parser.add_argument('--append', action='store_true',
                        help='Add these workloads to an existing --output, as a second pair of engines (a build variant) does')
    parser.add_argument('--max-ratio', type=float, help='Fail if first/second exceeds this for any workload')
    parser.add_argument('--limits', type=Path, help='DIR of files named by workload, each its recorded first/second ratio; '
                        'fail if one is exceeded by more than noise (overrides --max-ratio per workload)')
    parser.add_argument('--ratchet', type=Path, nargs='+', metavar='RESULT',
                        help='Lower --limits in place to what every RESULT (an --output) beats; run nothing')
    args = parser.parse_args()
    if args.ratchet:
        if not args.limits:
            parser.error('--ratchet needs --limits')
        limits = read_limits(args.limits)
        lowered = ratchet([json.loads(path.read_text()) for path in args.ratchet], limits)
        for workload, record in lowered.items():
            print(f'{workload}: {limits[workload]:.2f} -> {record:.2f}')
            (args.limits / workload).write_text(f'{record:.2f}\n')
        return 0
    if not args.engine:
        parser.error('--engine is required')
    engines = parse_engines(args.engine, args.php, args.php_extension)
    if (args.max_ratio or args.limits) and len(engines) != 2:
        parser.error('--max-ratio and --limits need exactly two engines')
    recorded = read_limits(args.limits) if args.limits else {}
    results = json.loads(args.output.read_text())['instructions_per_request'] if args.append else {}
    failed = False
    for workload in args.workload or ['plaintext']:
        counts = results[workload] = {}
        profiles = args.profiles / workload if args.profiles else None
        for engine in engines:
            counts[engine.name] = instructions(engine, WORKLOADS[workload], args.requests, args.warmup, profiles)
            print(f'{engine.name:10} {workload:15} {counts[engine.name]:12.0f} instructions/request', flush=True)
        if len(counts) == 2:
            (first, a), (second, b) = counts.items()
            limit, over, stale = verdict(a / b, recorded.get(workload), args.max_ratio)
            failed |= over
            if over:
                annotate('error', f'callgrind {workload}', f'ratio {a / b:.4f} over limit {limit:.4f}')
            print(f'ratio {first}/{second} {workload}: {a / b:.4f}' + (f' exceeds {limit:.4f}' if over else '')
                  + (f'; below the recorded {recorded[workload]}, lowest record {lowest(a / b):.2f}' if stale else ''))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({'requests': args.requests, 'max_ratio': args.max_ratio,
                                       'instructions_per_request': results}, indent=2) + '\n')
    print('results: ' + str(args.output))
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
