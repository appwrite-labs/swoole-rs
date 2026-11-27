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


def test_url_rewrite_rules_warn_only_for_patterns_upstream_runs_and_this_server_cannot(engine):
    # A backreference or lookahead (#5277) warns at set(), naming the pattern; a pattern
    # std::regex rejects, a supported one, and a prefix stay silent, as upstream is.
    code = r'''
    $warnings = [];
    set_error_handler(static function (int $level, string $message) use (&$warnings) {
        $warnings[] = [$level, $message];
        return true;
    });
    $server = new Swoole\Http\Server('127.0.0.1', 0, SWOOLE_BASE);
    foreach (['~(a)\1~', '~a(?=b)~', '~^/user/(\d+)$~', '~[~', '~(?=a{100000})~', '/prefix/'] as $pattern) {
        $warnings = [];
        $server->set(['url_rewrite_rules' => [$pattern => '/x']]);
        echo json_encode([$pattern, $warnings]), "\n";
    }
    '''
    result = subprocess.run(engine.command('-r', code), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    rows = dict(json.loads(line) for line in result.stdout.splitlines())
    warned = {
        pattern: [
            [2, f"Swoole\\Server::set(): url_rewrite_rules: the pattern '{pattern}' uses a backreference "
                'or lookahead, which is not supported; the rule never matches']
        ]
        for pattern in ('~(a)\\1~', '~a(?=b)~')
    }
    assert rows == {**{pattern: [] for pattern in rows}, **warned}


@pytest.mark.parametrize("mode", ["configured", "default"])
def test_incremental_request_byte_limit(engine, mode):
    code = r'''
    $header = "POST / HTTP/1.1\r\nContent-Length: %d\r\n\r\n";
    if ($argv[1] === 'default') {
        $limit = 2 * 1024 * 1024;
        // Both the full request and its body have seven-digit lengths.
        $body = str_repeat('a', $limit - strlen(sprintf($header, $limit)));
        $r = Swoole\Http\Request::create();
    } else {
        $body = 'abc';
        $limit = strlen(sprintf($header, strlen($body))) + strlen($body);
        $r = Swoole\Http\Request::create(['package_max_length' => $limit]);
    }
    $head = sprintf($header, strlen($body));
    var_dump($r->parse($head) === strlen($head));
    var_dump($r->parse($body . 'x') === false, swoole_last_error() === SWOOLE_ERROR_PACKAGE_LENGTH_TOO_LARGE);
    var_dump($r->getData() === $head, !$r->isCompleted());
    var_dump($r->parse($body) === strlen($body), $r->isCompleted(), strlen($r->getData()) === $limit, $r->getContent() === $body);
    '''
    result = subprocess.run(engine.command('-r', code, mode), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.splitlines() == ['bool(true)'] * 9


def test_incremental_multipart_fields_reuse_buffer(engine):
    code = r'''
    $value = str_repeat("a\0b", 4096);
    $first = "--foo\r\nContent-Disposition: form-data; name=\"first\"\r\n\r\n";
    $rest = "\r\n--foo\r\nContent-Disposition: form-data; name=\"empty\"\r\n\r\n" .
            "\r\n--foo\r\nContent-Disposition: form-data; name=\"last\"\r\n\r\nz\r\n--foo--\r\n";
    $length = strlen($first) + strlen($value) + strlen($rest);
    $head = "POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=foo\r\nContent-Length: $length\r\n\r\n";
    $r = Swoole\Http\Request::create(['package_max_length' => strlen($head) + $length]);
    foreach ([$head . $first, ...str_split($value, 1000), $rest] as $piece) {
        if ($r->parse($piece) !== strlen($piece)) throw new RuntimeException('short parse');
    }
    var_dump($r->isCompleted(), $r->post === ['first' => $value, 'empty' => '', 'last' => 'z']);
    '''
    result = subprocess.run(engine.command('-r', code), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.splitlines() == ['bool(true)'] * 2


def test_multipart_file_is_registered_with_php(engine, tmp_path):
    code = r'''
    $body = "--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\n\r\nsample\r\n--b--\r\n";
    $head = "POST / HTTP/1.1\r\nContent-Type: multipart/form-data; boundary=b\r\nContent-Length: " . strlen($body) . "\r\n\r\n";
    $request = Swoole\Http\Request::create(['parse_files' => true]);
    $request->parse($head . $body);
    $file = $request->files['file']['tmp_name'];
    var_dump(is_uploaded_file($file), move_uploaded_file($file, $argv[1]), is_uploaded_file($file));
    unset($request);
    '''
    moved = tmp_path / 'moved.txt'
    result = subprocess.run(engine.command('-r', code, str(moved)), capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.stdout.splitlines() == ['bool(true)', 'bool(true)', 'bool(false)']
    assert moved.read_bytes() == b'sample'


def test_invalid_request_logs_receive_warning(engine, tmp_path):
    log = tmp_path / 'server.log'
    with server(engine, Path(__file__).with_name('header_reentry.php'),
                {'TEST_HTTP_WARNING_LOG': str(log)}) as (port, _):
        with socket.create_connection(('127.0.0.1', port), timeout=5) as conn:
            conn.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nMissingColon\r\n\r\n')
            with conn.makefile('rb') as response:
                assert response.readline() == b'HTTP/1.1 400 Bad Request\r\n'
        # The same worker has finished logging before it serves the next request.
        assert get(port, '/')[0] == 'HTTP/1.1 200 OK'
        assert ('\tWARNING\tphp_swoole_http_server_onReceive(): Invalid HTTP request discarded: '
                '4 bytes unprocessed. Reason: Invalid header token') in log.read_text()


def test_standalone_request_upload_uses_php_temp_directory(engine, tmp_path):
    code = r'''
    $body = "--part\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\n\r\nnotes\r\n--part--\r\n";
    $request = Swoole\Http\Request::create();
    $request->parse("POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=part\r\n"
        . "Content-Length: " . strlen($body) . "\r\n\r\n" . $body);
    $path = $request->files['file']['tmp_name'];
    echo json_encode([dirname($path), file_get_contents($path)]), "\n";
    unset($request);
    echo json_encode(file_exists($path));
    '''
    result = subprocess.run(engine.command('-d', f'sys_temp_dir={tmp_path}', '-r', code),
                            capture_output=True, text=True, check=True, timeout=10)
    assert [json.loads(line) for line in result.stdout.splitlines()] == [[str(tmp_path), 'notes'], False]


def test_response_create_rejects_missing_server_and_request_context(engine):
    code = r'''
    use Swoole\Http\Request;
    use Swoole\Http\Response;
    foreach ([1, [new stdClass, new Request]] as $target) {
        try {
            Response::create($target);
            echo "unexpected success\n";
        } catch (Swoole\Exception $e) {
            echo json_encode([$e->getCode() === SWOOLE_ERROR_INVALID_PARAMS, $e->getMessage()]), "\n";
        }
    }
    '''
    result = subprocess.run(engine.command('-r', code), capture_output=True, text=True,
                            check=True, timeout=10)
    assert [json.loads(line) for line in result.stdout.splitlines()] == [
        [True, r'failed to create Http\Response from the given arguments'],
        [True, r'parameter $1.second has no valid Http\Request context'],
    ]
