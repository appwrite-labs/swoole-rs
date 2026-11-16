"""A header or trailer value's __toString replaces the array being sent (#3263).

The response carries the array as it was when sending began, and the server keeps serving.
"""
from pathlib import Path
import json
import socket
import subprocess

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


def test_request_metadata_separates_the_current_property(engine):
    code = r'''
    foreach ([[], ['seed' => 7]] as $initial) {
        $request = Swoole\Http\Request::create();
        $request->parse("GET /path HTTP/1.1\r\nHost: localhost\r\n");
        $request->server = $initial;
        $copy = $request->server;
        $request->parse("\r\n");
        echo json_encode([$copy, $request->server]), "\n";
    }
    '''
    result = subprocess.run(engine.command('-r', code), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(rows) == 2
    for (copy, current), initial in zip(rows, [[], {'seed': 7}]):
        assert copy == initial
        assert list(current) == list(initial) + [
            'request_method', 'request_uri', 'path_info', 'request_time',
            'request_time_float', 'server_protocol',
        ]
        assert [current[key] for key in ('request_method', 'request_uri', 'path_info', 'server_protocol')] == [
            'GET', '/path', '/path', 'HTTP/1.1',
        ]
        assert isinstance(current['request_time'], int)
        assert isinstance(current['request_time_float'], float)


@pytest.mark.parametrize(('method', 'protocol'), [
    ('GET', 'HTTP/1.1'), ('POST', 'HTTP/1.1'), ('M-SEARCH', 'HTTP/1.1'),
    ('GET_PARAMETER', 'RTSP/1.0'), ('QUERY', 'HTTP/1.1'),
])
def test_request_method_strings_separate_on_write(engine, method, protocol):
    code = r'''
    function request($method, $protocol) {
        $request = Swoole\Http\Request::create();
        $request->parse("$method / $protocol\r\nHost: localhost\r\n\r\n");
        return $request;
    }
    $first = request($argv[1], $argv[2]);
    $second = request($argv[1], $argv[2]);
    $copy = $first->server['request_method'];
    $first->server['request_method'][0] = '!';
    $third = request($argv[1], $argv[2]);
    echo json_encode([$copy, $first->server['request_method'],
                      $second->server['request_method'], $third->server['request_method']]);
    '''
    result = subprocess.run(engine.command('-r', code, method, protocol), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    assert json.loads(result.stdout) == [method, '!' + method[1:], method, method]
