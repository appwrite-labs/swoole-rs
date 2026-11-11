#!/usr/bin/env python3
"""Appwrite endpoint load, stock (upstream) against hybrid Swoole.

  appwrite_bench.py run VARIANT TRIAL OUT   load the running stack once, write OUT/VARIANT-TRIAL.json
  appwrite_bench.py report OUT              Markdown medians across trials, per endpoint and variant

`run` provisions a project on the fresh stack (console account, team, project, API
key, user, collection, bucket and one file), drives each endpoint with ApacheBench
(keep-alive, fixed request count and concurrency) from the host, opens realtime
subscriptions with a minimal WebSocket client, and samples the memory of every container
running the image under test (`appwrite-dev`: the API, realtime and the workers) with
`docker stats` after the load. appwrite-e2e.yml alternates the variants
on one runner so both see the same machine; the stack needs _APP_OPTIONS_ABUSE
disabled, or session creation is rate limited.
"""
import base64
import json
import os
from pathlib import Path
import re
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

URL = 'http://localhost/v1'
REQUESTS, CONCURRENCY = 5000, 32
# Writes hash passwords or store files, so they get fewer requests.
WRITES = 500
SUBSCRIPTIONS = 200
EMAIL, PASSWORD = 'bench@example.com', 'bench-password-1'
SCOPES = ['users.read', 'users.write', 'databases.read', 'databases.write', 'collections.read',
          'collections.write', 'documents.read', 'documents.write', 'buckets.read', 'buckets.write',
          'files.read', 'files.write']


def call(method, path, body=None, headers=None, raw=None, content_type='application/json'):
    """One API call: (status, parsed JSON body, response headers); raises on a non-2xx status."""
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    request = urllib.request.Request(URL + path, data=data, method=method,
                                     headers={'Content-Type': content_type, 'Origin': 'http://localhost', **(headers or {})})
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            text = response.read()
            return response.status, json.loads(text) if text else {}, response.headers
    except urllib.error.HTTPError as error:
        raise RuntimeError(f'{method} {path}: {error.code} {error.read()[:500]!r}') from None


def multipart(name, content):
    boundary = 'swoole-rs-bench'
    body = (f'--{boundary}\r\nContent-Disposition: form-data; name="fileId"\r\n\r\nunique()\r\n'
            f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
            f'Content-Type: application/octet-stream\r\n\r\n').encode() + content + f'\r\n--{boundary}--\r\n'.encode()
    return body, f'multipart/form-data; boundary={boundary}'


def provision():
    """A project with an API key, a user, a collection, a bucket and one stored file."""
    console = {'X-Appwrite-Project': 'console'}
    call('POST', '/account', {'userId': 'unique()', 'email': EMAIL, 'password': PASSWORD, 'name': 'bench'}, console)
    _, _, headers = call('POST', '/account/sessions/email', {'email': EMAIL, 'password': PASSWORD}, console)
    cookie = '; '.join(value.split(';')[0] for value in headers.get_all('Set-Cookie') or [])
    console['Cookie'] = cookie
    _, team, _ = call('POST', '/teams', {'teamId': 'unique()', 'name': 'bench'}, console)
    for attempt in range(5):
        try:
            _, project, _ = call('POST', '/projects', {'projectId': 'unique()', 'region': 'default', 'name': 'bench',
                                                       'teamId': team['$id']}, console)
            break
        except RuntimeError:
            if attempt == 4:
                raise
            time.sleep(1)
    _, key, _ = call('POST', f'/projects/{project["$id"]}/keys',
                     {'keyId': 'unique()', 'name': 'bench', 'scopes': SCOPES}, console)
    admin = {'X-Appwrite-Project': project['$id'], 'X-Appwrite-Key': key['secret']}
    call('POST', '/users', {'userId': 'unique()', 'email': EMAIL, 'password': PASSWORD}, admin)
    call('POST', '/databases', {'databaseId': 'bench', 'name': 'bench'}, admin)
    call('POST', '/databases/bench/collections', {'collectionId': 'items', 'name': 'items', 'documentSecurity': False,
                                                  'permissions': ['read("any")', 'create("any")']}, admin)
    call('POST', '/databases/bench/collections/items/attributes/string',
         {'key': 'title', 'size': 64, 'required': False}, admin)
    for _ in range(60):
        if call('GET', '/databases/bench/collections/items/attributes/title', headers=admin)[1].get('status') == 'available':
            break
        time.sleep(1)
    else:
        raise RuntimeError('attribute title never became available')
    call('POST', '/databases/bench/collections/items/documents', {'documentId': 'unique()', 'data': {'title': 'seed'}}, admin)
    call('POST', '/storage/buckets', {'bucketId': 'bench', 'name': 'bench', 'fileSecurity': False,
                                      'permissions': ['read("any")', 'create("any")']}, admin)
    body, content_type = multipart('seed.bin', os.urandom(4096))
    _, file, _ = call('POST', '/storage/buckets/bench/files', headers=admin, raw=body, content_type=content_type)
    return project['$id'], key['secret'], file['$id']


def endpoints(project, secret, file, scratch):
    """name -> (path, ab options, requests); POST bodies are written under scratch."""
    def post(name, body, content_type='application/json'):
        path = scratch / name
        path.write_bytes(body if isinstance(body, bytes) else json.dumps(body).encode())
        return ['-p', str(path), '-T', content_type]
    key = ['-H', f'X-Appwrite-Project: {project}', '-H', f'X-Appwrite-Key: {secret}']
    upload, upload_type = multipart('bench.bin', os.urandom(4096))
    return {
        # Public health, and a project-scoped read that goes through Appwrite's router and auth.
        'health-version': ('/health/version', [], REQUESTS),
        'locale-countries': ('/locale/countries', ['-H', 'X-Appwrite-Project: console'], REQUESTS),
        'session-create': ('/account/sessions/email', ['-H', f'X-Appwrite-Project: {project}',
                           *post('session', {'email': EMAIL, 'password': PASSWORD})], WRITES),
        'document-create': ('/databases/bench/collections/items/documents',
                            [*key, *post('document', {'documentId': 'unique()', 'data': {'title': 'bench'}})], WRITES),
        'document-list': ('/databases/bench/collections/items/documents', key, REQUESTS),
        'file-upload': ('/storage/buckets/bench/files', [*key, *post('file', upload, upload_type)], WRITES),
        'file-download': (f'/storage/buckets/bench/files/{file}/download', key, REQUESTS),
    }


def subscribe(project):
    """Seconds to open one realtime subscription and receive its `connected` event."""
    start = time.perf_counter()
    with socket.create_connection(('localhost', 80), timeout=10) as connection:
        nonce = base64.b64encode(os.urandom(16)).decode()
        connection.sendall((f'GET /v1/realtime?project={project}&channels%5B%5D=documents HTTP/1.1\r\n'
                            f'Host: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n'
                            f'Sec-WebSocket-Key: {nonce}\r\nSec-WebSocket-Version: 13\r\n\r\n').encode())
        received = b''
        while b'"connected"' not in received:
            chunk = connection.recv(4096)
            if not chunk:
                raise RuntimeError(f'realtime closed before connected: {received[:300]!r}')
            received += chunk
        if not received.startswith(b'HTTP/1.1 101'):
            raise RuntimeError(f'realtime handshake: {received[:300]!r}')
    return time.perf_counter() - start


def realtime(project):
    times, failed, start = [], 0, time.perf_counter()
    for _ in range(SUBSCRIPTIONS):
        try:
            times.append(subscribe(project))
        except (OSError, RuntimeError):
            failed += 1
    elapsed = time.perf_counter() - start
    times.sort()
    percentile = lambda q: times[min(len(times) - 1, int(q * len(times)))] * 1000 if times else 0.0
    return {'requests_per_second': SUBSCRIPTIONS / elapsed, 'p50_ms': percentile(0.5), 'p99_ms': percentile(0.99),
            'failed': failed, 'non_2xx': 0}


def ab(path, headers, requests=REQUESTS):
    command = ['ab', '-q', '-k', '-n', str(requests), '-c', str(CONCURRENCY), *headers, URL + path]
    output = subprocess.run(command, capture_output=True, text=True, check=True).stdout
    number = lambda pattern: float(re.search(pattern, output, re.M).group(1))
    return {'requests_per_second': number(r'^Requests per second:\s+([\d.]+)'),
            'p50_ms': number(r'^\s+50%\s+(\d+)'), 'p99_ms': number(r'^\s+99%\s+(\d+)'),
            # ab also counts a body whose length differs from the first (every new document's
            # does) as failed; non_2xx still reports error responses.
            'failed': int(number(r'^Failed requests:\s+(\d+)'))
            - int((re.search(r'\(Connect: \d+, Receive: \d+, Length: (\d+)', output) or [0, 0])[1]),
            'non_2xx': int(number(r'^Non-2xx responses:\s+(\d+)')) if 'Non-2xx' in output else 0}


def memory_mib():
    """{container: MiB in use} for every container running the image under test."""
    names = subprocess.run(['docker', 'ps', '--filter', 'ancestor=appwrite-dev', '--format', '{{.Names}}'],
                           capture_output=True, text=True, check=True).stdout.split()
    stats = subprocess.run(['docker', 'stats', '--no-stream', '--format', '{{.Name}} {{.MemUsage}}', *names],
                           capture_output=True, text=True, check=True).stdout.splitlines()
    memory = {}
    for line in stats:
        name, value, unit = re.match(r'(\S+) ([\d.]+)\s*([KMG]i?B)', line).groups()
        memory[name] = float(value) * {'KiB': 1 / 1024, 'MiB': 1, 'GiB': 1024, 'KB': 1 / 1000, 'MB': 1, 'GB': 1000}[unit]
    return memory


def run(variant, trial, out):
    project, secret, file = provision()
    with tempfile.TemporaryDirectory() as scratch:
        results = {name: ab(path, options, requests)
                   for name, (path, options, requests) in endpoints(project, secret, file, Path(scratch)).items()}
    results['realtime-subscribe'] = realtime(project)
    record = {'variant': variant, 'trial': int(trial), 'requests': REQUESTS, 'concurrency': CONCURRENCY,
              'endpoints': results, 'memory_mib': memory_mib()}
    out.mkdir(parents=True, exist_ok=True)
    (out / f'{variant}-{trial}.json').write_text(json.dumps(record, indent=2) + '\n')
    print(json.dumps(record))


def report(out):
    records = [json.loads(path.read_text()) for path in sorted(out.glob('*.json'))]
    variants = sorted({record['variant'] for record in records})
    median = lambda variant, pick: statistics.median(pick(r) for r in records if r['variant'] == variant)
    names = list(records[0]['endpoints']) if records else []
    lines = [f'**Appwrite endpoint load** ({REQUESTS} requests, {WRITES} for writes, {SUBSCRIPTIONS} realtime '
             f'subscriptions, concurrency {CONCURRENCY}, median of {len(records) // max(len(variants), 1)} '
             'alternating trials)', '',
             '| endpoint | metric | ' + ' | '.join(variants) + ' |', '| --- | --- |' + ' --- |' * len(variants)]
    for name in names:
        for metric in ['requests_per_second', 'p50_ms', 'p99_ms', 'failed', 'non_2xx']:
            values = [f"{median(v, lambda r: r['endpoints'][name][metric]):.1f}" for v in variants]
            lines.append(f'| {name} | {metric} | ' + ' | '.join(values) + ' |')
    containers = sorted({name for record in records for name in record['memory_mib']})
    for name in containers + ['all containers']:
        def cell(variant):
            mine = [r['memory_mib'] for r in records if r['variant'] == variant]
            values = [sum(m.values()) if name == 'all containers' else m[name] for m in mine if name in m or name == 'all containers']
            return f'{statistics.median(values):.0f}' if values else '–'
        lines.append(f'| {name} | memory MiB | ' + ' | '.join(cell(v) for v in variants) + ' |')
    print('\n'.join(lines))
    return int(any(r['endpoints'][name]['failed'] or r['endpoints'][name]['non_2xx']
                   for r in records for name in names))


if __name__ == '__main__':
    if sys.argv[1:2] == ['run'] and len(sys.argv) == 5:
        run(sys.argv[2], sys.argv[3], Path(sys.argv[4]))
    elif sys.argv[1:2] == ['report'] and len(sys.argv) == 3:
        sys.exit(report(Path(sys.argv[2])))
    else:
        sys.exit(__doc__)
