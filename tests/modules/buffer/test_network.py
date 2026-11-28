"""Verify ordered streaming, queued output, sendfile offsets and keep-alive."""
import http.client
import json
from pathlib import Path
import struct
import time

from tools.harness import server


def test_network(engine):
    """Large and file responses through the bounded output queue, a cancelled partial
    response, and health afterwards, on one keep-alive connection."""
    block = bytes(range(256)) * 256
    payload = b''.join(struct.pack('>I', i) + block[4:] for i in range(64))
    with server(engine, Path(__file__).with_name('server.php')) as (port, process):
        conn = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
        try:
            for path, expected in [('/stream', payload), ('/file', payload[13:-14]), ('/', b'ok')]:
                conn.request('GET', path)
                # Let the bounded server queue fill before receiving the body.
                time.sleep(.1)
                response = conn.getresponse()
                body = bytearray()
                while chunk := response.read(16384):
                    body.extend(chunk)
                assert response.status == 200 and body == expected, f'{path}: status={response.status}, bytes={len(body)}'
                assert conn.sock is not None, 'keep-alive connection closed'
                if path == '/stream':
                    conn.request('GET', '/queue')
                    observed = conn.getresponse()
                    info = json.loads(observed.read())
                    assert observed.status == 200 and info.get('send_queued_bytes', 0) > 0, f'output queue was not exercised: {info}'
            cancelled = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
            try:
                cancelled.request('GET', '/stream')
                time.sleep(.05)
                partial = cancelled.getresponse()
                assert partial.status == 200 and len(partial.read(1024)) == 1024, 'partial response failed'
            finally:
                cancelled.close()
            time.sleep(.05)
            conn.request('GET', '/')
            healthy = conn.getresponse()
            assert healthy.status == 200 and healthy.read() == b'ok', 'server failed after cancelled response'
            assert process.poll() is None, 'server exited'
        finally:
            conn.close()
