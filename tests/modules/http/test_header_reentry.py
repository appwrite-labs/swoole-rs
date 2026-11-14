"""A header or trailer value's __toString replaces the array being sent (#3263).

The response carries the array as it was when sending began, and the server keeps serving.
"""
from pathlib import Path
import socket

import pytest

from tools.harness import server

HEADERS = [('x-a', 'a'), ('x-list', 'l1'), ('x-list', 'l2'), ('x-b', 'b')]


def get(port, path):
    with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
        conn.sendall(f'GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n'.encode())
        data = b''
        while chunk := conn.recv(65536):
            data += chunk
    head, _, body = data.partition(b'\r\n\r\n')
    status, *lines = head.decode('latin-1').split('\r\n')
    fields = [(name.strip().lower(), value.strip()) for name, _, value in (line.partition(':') for line in lines)]
    return status, fields, body


@pytest.mark.parametrize('path', ['/replace-empty', '/replace-new', '/nested'])
def test_header_walk(engine, path):
    with server(engine, Path(__file__).with_name('header_reentry.php')) as (port, _):
        status, fields, body = get(port, path)
        assert status == 'HTTP/1.1 200 OK'
        assert [field for field in fields if field[0].startswith('x-')] == HEADERS
        assert body == b'ok'
        assert get(port, '/')[0] == 'HTTP/1.1 200 OK'


def test_trailer_walk(engine):
    with server(engine, Path(__file__).with_name('header_reentry.php')) as (port, _):
        status, _, body = get(port, '/trailer')
        assert status == 'HTTP/1.1 200 OK'
        assert body == b'5\r\nchunk\r\n0\r\nx-t1: t1\r\nx-t2: t2\r\n\r\n'
        assert get(port, '/')[0] == 'HTTP/1.1 200 OK'
